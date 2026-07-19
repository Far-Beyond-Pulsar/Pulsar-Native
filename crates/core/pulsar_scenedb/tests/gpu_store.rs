//! M2a headless verification (design Rev 3 §9): real surfaceless wgpu device;
//! the test harness owns the `device.poll` pump.

use pulsar_scenedb::gpu::EngineGpuContext;
use pulsar_scenedb::gpu::SceneBuffer;
use pulsar_scenedb::gpu::DirtyMask;
use pulsar_scenedb::gpu::{CellSlot, FrameDriver, RegionClassConfig, SceneGpuConfig, SceneGpuStore, SimulateA};
use pulsar_scenedb::{CellStorage, CellType, InstanceInfo, TypeToken};
use std::sync::Arc;

fn mat(seed: f32) -> [f32; 16] {
    core::array::from_fn(|i| seed + i as f32)
}

fn as_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn as_u32s(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

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
        label: Some("scenedb-m2a-test"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
}

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

#[test]
fn smoke_device_and_readback() {
    let ctx = test_context();
    let buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    ctx.queue().write_buffer(&buf, 0, &[7u8; 16]);
    assert_eq!(readback(&ctx, &buf, 16), vec![7u8; 16]);
}

use pulsar_scenedb::gpu::SubmissionTracker;

#[test]
fn tracker_serials_are_monotonic_and_start_incomplete() {
    let t = SubmissionTracker::new();
    let s1 = t.next_serial();
    let s2 = t.next_serial();
    assert_eq!((s1, s2), (1, 2));
    assert_eq!(t.completed(), 0, "nothing complete before any signal");
    t.force_complete(s1);
    assert_eq!(t.completed(), 1);
    t.force_complete(0); // watermark never regresses
    assert_eq!(t.completed(), 1);
}

#[test]
fn tracker_real_gpu_completion_path() {
    let ctx = test_context();
    let t = SubmissionTracker::new();
    let s = t.next_serial();
    ctx.queue().submit([]); // empty submission is enough to complete
    t.signal_submitted(ctx.queue(), s);
    ctx.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    assert!(t.completed() >= s, "on_submitted_work_done raised the watermark");
}

#[test]
fn delta_correctness_gpu_bytes_match_cpu_column() {
    let ctx = test_context();
    let buf = SceneBuffer::<[f32; 16]>::new(ctx.device(), "instances", 8);
    let dirty = DirtyMask::new(8);
    let cpu: Vec<[f32; 16]> = (0..4).map(|i| mat(i as f32 * 100.0)).collect();
    for row in 0..4 {
        dirty.mark(row);
    }
    let stats = buf.sync_region(ctx.queue(), &cpu, 0, &dirty);
    assert_eq!(stats.ranges, 1, "4 contiguous dirty rows coalesce into one write");
    assert_eq!(stats.bytes, 4 * 64);
    let gpu = as_f32s(&readback(&ctx, buf.buffer(), 4 * 64));
    let expect: Vec<f32> = cpu.iter().flatten().copied().collect();
    assert_eq!(gpu, expect, "GPU bytes == CPU transform column, by row");
}

#[test]
fn delta_minimality_clean_frame_writes_nothing_and_scattered_rows_coalesce() {
    let ctx = test_context();
    let buf = SceneBuffer::<[f32; 16]>::new(ctx.device(), "instances", 64);
    let dirty = DirtyMask::new(64);
    let cpu: Vec<[f32; 16]> = (0..64).map(|i| mat(i as f32)).collect();
    // Warm upload.
    for row in 0..64 {
        dirty.mark(row);
    }
    buf.sync_region(ctx.queue(), &cpu, 0, &dirty);
    // Zero-mutation frame writes nothing.
    let stats = buf.sync_region(ctx.queue(), &cpu, 0, &dirty);
    assert_eq!((stats.ranges, stats.bytes), (0, 0), "clean frame is free");
    // Scattered dirty rows: {3}, {10,11,12}, {60} → exactly 3 ranges.
    for row in [3u32, 10, 11, 12, 60] {
        dirty.mark(row);
    }
    let stats = buf.sync_region(ctx.queue(), &cpu, 0, &dirty);
    assert_eq!(stats.ranges, 3, "contiguous runs coalesce; no clean-row uploads");
    assert_eq!(stats.bytes, 5 * 64);
}

/// M3-b T3 (R-PERF-1 measured decision, REJECT — see
/// `pulsar_scenedb::gpu::GAP_MERGE_THRESHOLD`'s doc): pins the T2 alloc-gate
/// scope note's claim that was previously asserted only in prose and never
/// tested — `sync_region`'s coalescing at `GAP_MERGE_THRESHOLD == 0` is
/// STRICT adjacency, meaning a SINGLE clean row between two dirty rows is
/// already enough to split them into two ranges (it does not take a run of
/// several clean rows). The `assert_eq!` on the constant itself is
/// deliberate: if a future change ever ships a nonzero G, this test must
/// fail loudly and be revisited rather than silently asserting behavior the
/// binary no longer has.
#[test]
fn sync_region_gap_of_one_row_splits_at_g0() {
    assert_eq!(
        pulsar_scenedb::gpu::GAP_MERGE_THRESHOLD, 0,
        "this test pins G=0's strict-adjacency behavior — update it (not just this assert) \
         if the shipped gap threshold ever changes"
    );
    let ctx = test_context();
    let buf = SceneBuffer::<[f32; 16]>::new(ctx.device(), "gap-test", 8);
    let dirty = DirtyMask::new(8);
    // Rows 2 and 4 dirty; row 3 (the ONLY row between them) clean.
    dirty.mark(2);
    dirty.mark(4);
    let cpu: Vec<[f32; 16]> = (0..8).map(|i| mat(i as f32)).collect();
    let stats = buf.sync_region(ctx.queue(), &cpu, 0, &dirty);
    assert_eq!(
        stats.ranges, 2,
        "a ONE-row gap between two dirty rows must still split into two ranges at G=0"
    );
    assert_eq!(stats.bytes, 2 * 64, "only the two dirty rows' bytes upload — the clean gap row is not re-uploaded");
    let gpu = as_f32s(&readback(&ctx, buf.buffer(), 8 * 64));
    // Row 3 (never dirtied, never uploaded) must read back as all-zero
    // (SceneBuffer is freshly allocated, never written) — proof the gap row
    // was genuinely skipped, not silently bridged.
    let row3 = &gpu[3 * 16..4 * 16];
    assert!(row3.iter().all(|&f| f == 0.0), "un-uploaded gap row must stay at its pre-sync (zero) bytes");
}

use pulsar_scenedb::gpu::GenerationBuffer;

#[test]
fn generation_buffer_write_and_rebuild() {
    let ctx = test_context();
    let gens = GenerationBuffer::new(ctx.device(), 4);
    gens.rebuild(ctx.queue(), &[1, 5, u32::MAX, 2]);
    assert_eq!(as_u32s(&readback(&ctx, gens.buffer(), 16)), vec![1, 5, u32::MAX, 2]);
    gens.write(ctx.queue(), 1, 6); // retirement bumps slot 1
    assert_eq!(as_u32s(&readback(&ctx, gens.buffer(), 16)), vec![1, 6, u32::MAX, 2]);
}

fn transform_cell(capacity: u32) -> CellStorage {
    let ct = CellType::new("m2a-instance")
        .with(TypeToken::of::<[f32; 16]>())
        .build()
        .unwrap();
    CellStorage::from_cell_type(&ct, capacity).unwrap()
}

/// M3-α T4: a `CellType`-based cell carrying BOTH the `[f32; 16]` transform
/// column and the `InstanceInfo` column, for tests that exercise
/// `write_instance_info`/the instance-info mirror without pulling in
/// `SpatialCell`'s bounds columns.
fn transform_info_cell(capacity: u32) -> CellStorage {
    let ct = CellType::new("m3a-instance-info")
        .with(TypeToken::of::<[f32; 16]>())
        .with(TypeToken::of::<InstanceInfo>())
        .build()
        .unwrap();
    CellStorage::from_cell_type(&ct, capacity).unwrap()
}

fn as_infos(bytes: &[u8]) -> Vec<InstanceInfo> {
    bytes
        .chunks_exact(8)
        .map(|c| InstanceInfo {
            mesh_index: u32::from_le_bytes(c[0..4].try_into().unwrap()),
            flags: u32::from_le_bytes(c[4..8].try_into().unwrap()),
        })
        .collect()
}

fn scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    }
}

/// Drives one full frame boundary through the compile-time phase machine
/// (T11, design Rev 2 §6) — the only path available to callers outside this
/// crate now that `retire_all`/`compact_all`/`sync_all` are `pub(crate)`.
/// Consumes the current `SimulateA` witness BY VALUE through the full
/// Simulate→Harvest→Boundary chain, and only then — after the boundary has
/// run — begins the next frame and returns its fresh `SimulateA`. The
/// next-frame witness is never manufactured while the old one is still live
/// (the hoarding pattern the phase machine discourages).
fn scene_boundary(
    frames: &mut FrameDriver,
    sim: SimulateA,
    store: &mut SceneGpuStore,
    slots: &mut [CellSlot<'_>],
) -> (SimulateA, pulsar_scenedb::gpu::SyncStats) {
    let stats = sim.end().end().end().run(store, slots);
    (frames.begin(), stats)
}

#[test]
fn write_transform_is_the_single_mutation_path() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let h = cell.alloc().unwrap();
    assert!(store.write_transform(id, &mut cell, h, &mat(9.0), &sim));
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    let row = cell.row_of(h).unwrap() as usize;
    let base = store.row_region_base(id) as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), (64 * 4 * 64) as u64));
    assert_eq!(&gpu[(base + row) * 16..(base + row) * 16 + 16], &mat(9.0));
    // Stale handle rejected.
    let dead = cell.alloc().unwrap();
    cell.free(dead);
    assert!(!store.write_transform(id, &mut cell, dead, &mat(0.0), &sim));
}

#[test]
fn compaction_move_is_resynced_and_generation_buffer_matches_registry() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(id, &mut cell, h, &mat(s), &sim);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    // Free hb via the deferred path; complete its serial; boundary again:
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(id, &mut cell, hb, serial, &sim));
    store.tracker().force_complete(serial);
    let stats = {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        // Last frame of the test: the returned next-frame witness is dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots).1 // retire → compact (hc moves) → sync
    };
    assert!(stats.ranges >= 1, "the compaction move was re-uploaded");
    // Moved row's GPU bytes are correct at its NEW index:
    let hc_row = cell.row_of(hc).unwrap() as usize;
    let base = store.row_region_base(id) as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), (64 * 4 * 64) as u64));
    assert_eq!(&gpu[(base + hc_row) * 16..(base + hc_row) * 16 + 16], &mat(3.0));
    // Generation buffer matches the registry for every allocated slot:
    let regs = cell.registry().generations().to_vec();
    let gpu_gens = as_u32s(&readback(&ctx, store.generation_buffer(), 64 * 4));
    let slot_base = 0usize; // slot region base for the first class-0 cell
    assert_eq!(&gpu_gens[slot_base..slot_base + regs.len()], &regs[..]);
}

/// M3-α T4 (C5 amendment): `write_instance_info` mirrors `write_transform`'s
/// machinery byte-exactly — write, drive the frame boundary, and read the
/// instance-info SSBO back to confirm it matches the CPU-side values row for
/// row. Also proves the write does NOT touch the generation buffer (the
/// stamp stays transform-only — see `write_instance_info`'s doc).
#[test]
fn write_instance_info_boundary_readback_is_byte_exact() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_info_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let h = cell.alloc().unwrap();
    let info = InstanceInfo { mesh_index: 7, flags: 1 };
    assert!(store.write_instance_info(id, &mut cell, h, info, &sim));
    // The instance-info write alone must not stamp a generation (that stamp
    // is transform-only, per `write_instance_info`'s doc).
    assert_eq!(store.generation_write_count(), 0, "write_instance_info never stamps the generation buffer");
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    let row = cell.row_of(h).unwrap() as usize;
    let base = store.row_region_base(id) as usize;
    let gpu = as_infos(&readback(&ctx, store.instance_info_buffer(), (64 * 4 * 8) as u64));
    assert_eq!(gpu[base + row], info, "GPU bytes == CPU instance-info column, by row");
    // Stale handle rejected, mirroring write_transform's contract.
    let dead = cell.alloc().unwrap();
    cell.free(dead);
    assert!(!store.write_instance_info(id, &mut cell, dead, info, &sim));
}

/// M3-α T4: mirrors `compaction_move_is_resynced_and_generation_buffer_
/// matches_registry`'s shape — a compaction-moved row must carry its
/// instance-info bytes to the new index, exactly like the transform column.
#[test]
fn compaction_move_carries_instance_info() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_info_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(id, &mut cell, h, &mat(s), &sim);
    }
    let infos = [(ha, 10u32), (hb, 20u32), (hc, 30u32)];
    for (h, mesh_index) in infos {
        store.write_instance_info(id, &mut cell, h, InstanceInfo { mesh_index, flags: 0 }, &sim);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    // Free hb via the deferred path; complete its serial; boundary again:
    // hc swaps into hb's vacated row (row 1).
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(id, &mut cell, hb, serial, &sim));
    store.tracker().force_complete(serial);
    let stats = {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut frames, sim, &mut store, &mut slots).1 // retire → compact (hc moves) → sync
    };
    assert!(stats.ranges >= 1, "the compaction move was re-uploaded");
    let hc_row = cell.row_of(hc).unwrap() as usize;
    let base = store.row_region_base(id) as usize;
    let gpu = as_infos(&readback(&ctx, store.instance_info_buffer(), (64 * 4 * 8) as u64));
    assert_eq!(
        gpu[base + hc_row],
        InstanceInfo { mesh_index: 30, flags: 0 },
        "moved row's instance-info bytes follow the compaction move, like the transform column"
    );
}

#[test]
fn generation_uploads_are_shadow_gated_to_changes_only() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let h = cell.alloc().unwrap();
    // Same write window: two transform writes, one generation upload.
    assert!(store.write_transform(id, &mut cell, h, &mat(1.0), &sim));
    assert!(store.write_transform(id, &mut cell, h, &mat(2.0), &sim));
    assert_eq!(
        store.generation_write_count(),
        1,
        "repeat writes to a live handle upload its generation exactly once"
    );
    // Next frame: a moving object's write is still generation-silent.
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    assert!(store.write_transform(id, &mut cell, h, &mat(3.0), &sim));
    assert_eq!(
        store.generation_write_count(),
        1,
        "unchanged generation is never re-uploaded across frames"
    );
    // Retirement bumps the generation → exactly one more upload. Split the
    // boundary into its individually-consuming stages (`BoundaryPhase`,
    // `RetiredPhase`) so the assert below lands strictly BETWEEN retire and
    // compact/sync, same as before the phase machine existed.
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(id, &mut cell, h, serial, &sim));
    store.tracker().force_complete(serial);
    // Consume the witness by value — the next frame's witness is not begun
    // until the boundary completes (no hoarding).
    let (retired, drained) = sim.end().end().end().retire(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    assert_eq!(drained, 1, "exactly one entry drained");
    assert_eq!(store.generation_write_count(), 2, "retirement writes the bumped generation");
    // Close the frame boundary (phase machine: retire → compact → sync).
    let compacted = retired.compact(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    compacted.sync(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
}

/// Test 6 host-side (design §9): the retirement invariant. A slot is never
/// reissued, and its row never reclaimed, before its serial completes and the
/// new generation is in the VRAM buffer; the handle stays row-resolvable but
/// harvest-dead during the window; afterwards it is rejected. No UB.
///
/// The between-stage asserts go through the staged boundary transitions
/// (`BoundaryPhase::retire` → `RetiredPhase::compact` → `CompactedPhase::sync`).
/// `retire` returns the drain count directly — asserted as the primary
/// observable — and `generation_write_count()` is kept as a second,
/// independent instrument for the same fact (retirement always bumps a
/// slot's generation, so the shadow-gated write count rises by exactly the
/// number of entries drained).
#[test]
fn test6_retirement_invariant() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let h = cell.alloc().unwrap();
    store.write_transform(id, &mut cell, h, &mat(42.0), &sim);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }

    let row = cell.row_of(h).unwrap();
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(id, &mut cell, h, serial, &sim));

    // Serial INCOMPLETE: boundary runs but nothing retires. The drain count
    // is the DIRECT observable; the generation-write-count delta is kept as
    // a second, independent instrument for the same fact.
    let gens_before = store.generation_write_count();
    let (retired, drained) = sim.end().end().end().retire(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    assert_eq!(drained, 0, "incomplete serial must not retire");
    assert_eq!(
        store.generation_write_count(),
        gens_before,
        "incomplete serial must not retire (no generation bump uploaded)"
    );
    let compacted = retired.compact(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    // Physical survival: the only occupied row is h's pinned row (h2 is not
    // alloc'd yet). A pin-ignoring compaction would tail-pop it to 0 without
    // touching the registry mapping, so row_of alone cannot catch that.
    assert_eq!(cell.rows_in_use(), 1, "pinned row physically survives compaction (only h's row)");
    assert_eq!(cell.row_of(h), Some(row), "row not compacted while pinned");
    compacted.sync(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    // Boundary complete — only now begin the next frame's witness.
    sim = frames.begin();
    // Still the incomplete-serial window (h's slot not yet reissued): the
    // write window is open again post-sync, but the handle is pending-retire
    // and must be rejected.
    assert!(!store.write_transform(id, &mut cell, h, &mat(0.0), &sim), "pending-retire handle must not be writable");
    let h2 = cell.alloc().unwrap();
    assert_ne!(h2.index(), h.index(), "slot not reissued while in flight");
    assert_eq!(cell.live_count(), 1, "pending row absent from harvest (only h2 lives)");

    // Serial COMPLETES: the drain writes VRAM gen BEFORE pooling the slot.
    store.tracker().force_complete(serial);
    let gens_before = store.generation_write_count();
    let (retired, drained) = sim.end().end().end().retire(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    assert_eq!(drained, 1, "completed serial retires exactly one entry");
    assert_eq!(store.generation_write_count(), gens_before + 1, "exactly one entry drained and its generation bumped");
    let gpu_gens = as_u32s(&readback(&ctx, store.generation_buffer(), 64 * 4));
    let slot_base = 0usize; // slot region base for the first class-0 cell
    assert_eq!(gpu_gens[slot_base + h.index() as usize], h.generation() + 1, "VRAM generation bumped");
    let compacted = retired.compact(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    compacted.sync(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
    assert_eq!(cell.row_of(h), None, "old handle rejected after retirement");
    let h3 = cell.alloc().unwrap();
    assert_eq!(h3.index(), h.index(), "slot recycled only now");
    assert_eq!(h3.generation(), h.generation() + 1);
}

/// Test 14 (C0 companion gate): drop the device + every buffer; create a
/// fresh device; rebuild the GPU side purely from Layer-1's authoritative
/// columns. Byte-identical recovery proves no GPU-only/derived scene state
/// exists (design §3 "derived data is not stored"). Also asserts the slot
/// mirror is byte-identical: `SceneGpuStore::rebuild` bulk-fills it up front
/// (no boundary has run yet to self-heal it lazily).
#[test]
fn test14_device_loss_rematerialization() {
    let cfg = scene_cfg();
    let mut cell = transform_cell(64);

    // Populate with churn so slot/row spaces diverge: alloc 8, retire 2.
    let ctx1 = test_context();
    let mut store = SceneGpuStore::new(&ctx1, cfg.clone());
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let hs: Vec<_> = (0..8).map(|_| cell.alloc().unwrap()).collect();
    for (i, &h) in hs.iter().enumerate() {
        store.write_transform(id, &mut cell, h, &mat(i as f32 * 10.0), &sim);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    for &h in &[hs[2], hs[5]] {
        let s = store.tracker().next_serial();
        store.free_deferred(id, &mut cell, h, s, &sim);
        store.tracker().force_complete(s);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        // Last frame before device loss: next-frame witness dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots);
    }
    let base_before = store.row_region_base(id) as usize;
    let before_rows = readback(&ctx1, store.transform_buffer(), (64 * 4 * 64) as u64);
    let before_gens = readback(&ctx1, store.generation_buffer(), 64 * 4);
    let before_mirror = readback(&ctx1, store.slot_mirror_buffer(), (64 * 4 * 4) as u64);

    // Device loss: drop the store, then the entire device.
    drop(store);
    drop(ctx1);

    // Fresh device; rebuild from CPU-authoritative columns only.
    let ctx2 = test_context();
    let (rebuilt, ids) = SceneGpuStore::rebuild(&ctx2, cfg, &[(0, &cell)]);
    let id2 = ids[0];
    let base_after = rebuilt.row_region_base(id2) as usize;
    let after_rows = readback(&ctx2, rebuilt.transform_buffer(), (64 * 4 * 64) as u64);
    let after_gens = readback(&ctx2, rebuilt.generation_buffer(), 64 * 4);
    let after_mirror = readback(&ctx2, rebuilt.slot_mirror_buffer(), (64 * 4 * 4) as u64);

    let rows = cell.rows_in_use() as usize;
    let n = rows * 64;
    let start_before = base_before * 64;
    let start_after = base_after * 64;
    assert_eq!(
        after_rows[start_after..start_after + n],
        before_rows[start_before..start_before + n],
        "row data byte-identical"
    );
    let s = cell.registry().generations().len() * 4;
    let slot_base = 0usize; // slot region base for the first class-0 cell
    assert_eq!(
        after_gens[slot_base..slot_base + s],
        before_gens[slot_base..slot_base + s],
        "generations byte-identical (incl. bumps)"
    );
    // Slot-mirror byte identity: `rebuild` bulk-fills the mirror before any
    // boundary self-heals it, so it must already match the pre-loss mirror
    // for every occupied row.
    let mirror_n = rows * 4;
    let mirror_start_before = base_before * 4;
    let mirror_start_after = base_after * 4;
    assert_eq!(
        after_mirror[mirror_start_after..mirror_start_after + mirror_n],
        before_mirror[mirror_start_before..mirror_start_before + mirror_n],
        "slot mirror byte-identical after rebuild (no boundary run yet)"
    );
}

#[test]
fn two_cells_write_into_disjoint_regions() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell_a = transform_cell(64);
    let mut cell_b = transform_cell(64);
    let ida = store.register_cell(&cell_a, 0).unwrap();
    let idb = store.register_cell(&cell_b, 0).unwrap();
    assert_ne!(store.row_region_base(ida), store.row_region_base(idb));
    let mut frames = FrameDriver::new();
    let sim = frames.begin();
    let ha = cell_a.alloc().unwrap();
    let hb = cell_b.alloc().unwrap();
    assert!(store.write_transform(ida, &mut cell_a, ha, &mat(1.0), &sim));
    assert!(store.write_transform(idb, &mut cell_b, hb, &mat(2.0), &sim));
    {
        let mut slots = [CellSlot { id: ida, cell: &mut cell_a }, CellSlot { id: idb, cell: &mut cell_b }];
        // Only frame of the test: next-frame witness dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots);
    }
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), (64 * 4 * 64) as u64));
    let base_a = store.row_region_base(ida) as usize;
    let base_b = store.row_region_base(idb) as usize;
    assert_eq!(&gpu[base_a * 16..base_a * 16 + 16], &mat(1.0), "cell A row 0 in region A");
    assert_eq!(&gpu[base_b * 16..base_b * 16 + 16], &mat(2.0), "cell B row 0 in region B");
}

#[test]
fn region_exhaustion_is_a_hard_error() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
            tombstone_headroom: 8,
            max_cells_metadata: 1,
        },
    );
    let c1 = transform_cell(64);
    let c2 = transform_cell(64);
    assert!(store.register_cell(&c1, 0).is_ok());
    assert!(store.register_cell(&c2, 0).is_err(), "second cell exceeds max_resident_cells");
}

#[test]
fn registration_rebuilds_generation_region_and_shadow() {
    // The D2 regression shape (single-region form; recycled-region form is β):
    // a cell with churned generations registers; its region must mirror the
    // registry immediately, with zero per-write stamps needed afterwards.
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let h1 = cell.alloc().unwrap();
    cell.free(h1); // immediate-free churn BEFORE registration: gen bumped to 2 in registry
    let h2 = cell.alloc().unwrap(); // recycles slot 0 at gen 2
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let sim = frames.begin();
    let gens = as_u32s(&readback(&ctx, store.generation_buffer(), 8));
    let sb = 0usize; // first slot region starts at 0
    assert_eq!(gens[sb], 2, "registration uploaded the churned generation");
    // Shadow seeded: writing the transform must NOT re-stamp the generation.
    let before = store.generation_write_count();
    assert!(store.write_transform(id, &mut cell, h2, &mat(3.0), &sim));
    assert_eq!(store.generation_write_count(), before, "shadow already knows gen 2");
}

/// M3-α T4 review, folded carry-forward (closed by M3-α T8's orchestration):
/// `register_cell`'s warm-up (`dirty_transforms.mark_range`/
/// `dirty_infos.mark_range`, scene_store.rs) is mutation-surviving in the
/// sense that no prior test ever registered a cell whose rows were already
/// populated CPU-side BEFORE registration — every existing test writes its
/// rows AFTER registration, via `write_transform`/`write_instance_info`,
/// whose OWN dirty-marking would mask a missing warm-up mark. This test
/// populates 3 rows directly through `CellStorage::column_for_mut` (bypassing
/// both mirrored-column writers entirely) BEFORE the cell is ever registered,
/// then runs one boundary with NO `write_transform`/`write_instance_info`
/// calls in it at all — the only thing that can put these rows' bytes in
/// VRAM is `register_cell`'s warm-up mark plus the first `sync_all`.
#[test]
fn register_cell_warmup_syncs_rows_populated_before_registration() {
    let ctx = test_context();
    let mut cell = transform_info_cell(64);

    // Populate 3 rows' worth of CPU-side truth BEFORE any `SceneGpuStore`
    // exists. `_handles` only needs to keep the rows live (alloc marks
    // liveness) — this test addresses rows by index, not by handle.
    let _handles: Vec<_> = (0..3).map(|_| cell.alloc().unwrap()).collect();
    let want_mats = [mat(11.0), mat(12.0), mat(13.0)];
    let want_infos = [
        InstanceInfo { mesh_index: 101, flags: 0 },
        InstanceInfo { mesh_index: 102, flags: 1 },
        InstanceInfo { mesh_index: 103, flags: 0 },
    ];
    {
        let cols = cell.column_for_mut::<[f32; 16]>().unwrap();
        for (row, m) in want_mats.iter().enumerate() {
            cols[row] = *m;
        }
    }
    {
        let cols = cell.column_for_mut::<InstanceInfo>().unwrap();
        for (row, info) in want_infos.iter().enumerate() {
            cols[row] = *info;
        }
    }

    // NOW register — this is the warm-up under test.
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let id = store.register_cell(&cell, 0).unwrap();
    let base = store.row_region_base(id) as usize;

    // One empty boundary: zero `write_transform`/`write_instance_info` calls
    // in this frame. If the sync uploads anything for these 3 rows, it can
    // only be because `register_cell`'s warm-up marked them dirty.
    let mut frames = FrameDriver::new();
    let sim = frames.begin();
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        // Only frame of the test: next-frame witness dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots);
    }

    let gpu_mats = as_f32s(&readback(&ctx, store.transform_buffer(), (64 * 4 * 64) as u64));
    for (row, want) in want_mats.iter().enumerate() {
        assert_eq!(
            &gpu_mats[(base + row) * 16..(base + row) * 16 + 16],
            want,
            "row {row}: pre-registration transform must reach VRAM via register_cell's warm-up"
        );
    }
    let gpu_infos = as_infos(&readback(&ctx, store.instance_info_buffer(), (64 * 4 * 8) as u64));
    for (row, want) in want_infos.iter().enumerate() {
        assert_eq!(
            gpu_infos[base + row], *want,
            "row {row}: pre-registration instance-info must reach VRAM via register_cell's warm-up"
        );
    }
}

#[test]
fn slot_mirror_tracks_alloc_and_compaction_moves() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(id, &mut cell, h, &mat(s), &sim);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    let base = store.row_region_base(id) as usize;
    let mirror = as_u32s(&readback(&ctx, store.slot_mirror_buffer(), (64 * 4 * 4) as u64));
    // slot region base for class-0 cell 0 is 0; global_slot == local slot here.
    assert_eq!(&mirror[base..base + 3], &[ha.index(), hb.index(), hc.index()]);
    // Retire hb; hc swaps into its row; the mirror must follow the move.
    let serial = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, hb, serial, &sim);
    store.tracker().force_complete(serial);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        // Last frame of the test: next-frame witness dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots);
    }
    let hc_row = cell.row_of(hc).unwrap() as usize;
    let mirror = as_u32s(&readback(&ctx, store.slot_mirror_buffer(), (64 * 4 * 4) as u64));
    assert_eq!(mirror[base + hc_row], hc.index(), "moved row's mirror entry updated");
}

/// Task 4 review regression (fail-open C6): a retired slot recycled into a
/// DIFFERENT row arrives with its generation already stamped by the retire,
/// so a gen-shadow-gated dirty trigger stays silent and the new row's mirror
/// entry keeps the previous occupant's slot — which VALIDATES against that
/// still-live slot's generation. The row-scoped slot shadow must catch it.
#[test]
fn slot_mirror_survives_slot_recycling_into_new_row() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(id, &mut cell, h, &mat(s), &sim);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    // Retire ha; hc swaps into ha's row (row 0); boundary uploads the move
    // and stamps ha's bumped generation into the gen-shadow.
    let serial = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, ha, serial, &sim);
    store.tracker().force_complete(serial);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    // Alloc recycles ha's slot — but into a NEW row (the tail), not ha's old
    // row, which hc now occupies.
    let hd = cell.alloc().unwrap();
    assert_eq!(hd.index(), ha.index(), "precondition: hd recycled ha's slot");
    let hd_row = cell.row_of(hd).unwrap() as usize;
    let hc_row = cell.row_of(hc).unwrap() as usize;
    assert_ne!(hd_row, hc_row, "precondition: recycled slot landed in a different row");
    store.write_transform(id, &mut cell, hd, &mat(4.0), &sim);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        // Last frame of the test: next-frame witness dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots);
    }
    let base = store.row_region_base(id) as usize;
    let mirror = as_u32s(&readback(&ctx, store.slot_mirror_buffer(), (64 * 4 * 4) as u64));
    // slot_base is 0 for the first class-0 cell — keep the explicit form.
    assert_eq!(mirror[base + hd_row], 0 + hd.index(), "recycled slot's new row must be re-uploaded");
    assert_eq!(mirror[base + hc_row], 0 + hc.index(), "moved row's mirror entry still correct");
}

/// Task 4 re-review regression (fail-open residual): alloc() into a row a
/// prior compaction vacated (rows_in_use shrank past it, then grew back),
/// never write_transform'd. Any write-path trigger never fires for it, so
/// mirror[row] would keep the MOVED prior occupant's slot — still live at
/// its matching generation — a ghost duplicate that VALIDATES. The sync_all
/// boundary scan must self-heal it.
#[test]
fn slot_mirror_self_heals_alloc_without_write() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(id, &mut cell, h, &mat(s), &sim);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    // Retire ha; hc swaps into row0; rows_in_use shrinks to 2 — row2 is
    // vacated but mirror[row2] still holds hc's slot (stale-but-inert while
    // unoccupied).
    let serial = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, ha, serial, &sim);
    store.tracker().force_complete(serial);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    // Re-occupy row2 with a recycled slot and DO NOT write its transform:
    // no write-path trigger can ever fire for this row.
    let hd = cell.alloc().unwrap();
    let hd_row = cell.row_of(hd).unwrap() as usize;
    assert_eq!(hd_row, 2, "precondition: hd re-occupied the vacated tail row");
    assert_ne!(hd.index(), hc.index(), "precondition: hd's slot differs from the stale mirror entry (non-vacuous)");
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        // Last frame of the test: next-frame witness dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots);
    }
    let base = store.row_region_base(id) as usize;
    let mirror = as_u32s(&readback(&ctx, store.slot_mirror_buffer(), (64 * 4 * 4) as u64));
    // slot_base is 0 for the first class-0 cell — keep the explicit form.
    assert_eq!(
        mirror[base + hd_row],
        0 + hd.index(),
        "boundary scan must self-heal the never-written re-occupied row"
    );
}

/// Test 14 extension (C0 companion, M2b-α scope): the single-cell form
/// (`test14_device_loss_rematerialization`) only proves recovery when a
/// cell's region happens to start at region base 0. Two cells force distinct
/// non-zero row/slot bases into the recovery path, and `SceneGpuStore::rebuild`
/// registers cells in argument order, so — for THIS test's two-cell,
/// single-class shape — the rebuilt store's bases land on the same offsets as
/// the original (both register cell A first, cell B second). Compare
/// region-relative slices regardless, per the design note: absolute buffer
/// equality is incidental, not the contract.
///
/// M3-α T4 extension: cells here also carry the `InstanceInfo` column
/// (`transform_info_cell`, not the bare `transform_cell`), churned alongside
/// the transform, so the instance-info SSBO's device-loss recovery is
/// asserted byte-identical too — same "derived data is not stored" gate
/// (design §3), now covering both mirrored columns.
#[test]
fn test14_multicell_device_loss_rematerialization() {
    let cfg = scene_cfg();
    // Region geometry, derived from `scene_cfg()` rather than hardcoded: the
    // row region size is exactly `capacity` (§7); the slot region adds the
    // tombstone headroom. With capacity=64, headroom=8, max_resident_cells=4:
    // slot_region_size = 72, so the second class-0 registrant's slot base is
    // 72 (first registrant's slot base is always 0).
    let row_capacity = cfg.classes[0].capacity;
    let headroom = cfg.tombstone_headroom;
    let slot_region_size = row_capacity + headroom;
    let max_resident = cfg.classes[0].max_resident_cells;
    let total_rows = (row_capacity * max_resident) as u64;
    let total_slots = (slot_region_size * max_resident) as u64;
    let transform_bytes = total_rows * 64;
    let mirror_bytes = total_rows * 4;
    let gen_bytes = total_slots * 4;
    let info_bytes = total_rows * 8;

    let mut cell_a = transform_info_cell(64);
    let mut cell_b = transform_info_cell(64);

    let ctx1 = test_context();
    let mut store = SceneGpuStore::new(&ctx1, cfg.clone());
    let id_a = store.register_cell(&cell_a, 0).unwrap();
    let id_b = store.register_cell(&cell_b, 0).unwrap();
    let mut frames = FrameDriver::new();
    let mut sim = frames.begin();

    // Churn each cell independently, with disjoint seed ranges so a
    // cross-cell mixup would not accidentally read back as correct.
    let hs_a: Vec<_> = (0..8).map(|_| cell_a.alloc().unwrap()).collect();
    for (i, &h) in hs_a.iter().enumerate() {
        assert!(store.write_transform(id_a, &mut cell_a, h, &mat(i as f32 * 10.0), &sim));
        assert!(store.write_instance_info(id_a, &mut cell_a, h, InstanceInfo { mesh_index: i as u32, flags: 0 }, &sim));
    }
    let hs_b: Vec<_> = (0..8).map(|_| cell_b.alloc().unwrap()).collect();
    for (i, &h) in hs_b.iter().enumerate() {
        assert!(store.write_transform(id_b, &mut cell_b, h, &mat(1000.0 + i as f32 * 10.0), &sim));
        assert!(store.write_instance_info(
            id_b,
            &mut cell_b,
            h,
            InstanceInfo { mesh_index: 1000 + i as u32, flags: 1 },
            &sim
        ));
    }
    {
        let mut slots = [CellSlot { id: id_a, cell: &mut cell_a }, CellSlot { id: id_b, cell: &mut cell_b }];
        sim = scene_boundary(&mut frames, sim, &mut store, &mut slots).0;
    }
    // Free 2 of 8 per cell via the deferred path; force-complete each serial.
    for &h in &[hs_a[2], hs_a[5]] {
        let s = store.tracker().next_serial();
        assert!(store.free_deferred(id_a, &mut cell_a, h, s, &sim));
        store.tracker().force_complete(s);
    }
    for &h in &[hs_b[2], hs_b[5]] {
        let s = store.tracker().next_serial();
        assert!(store.free_deferred(id_b, &mut cell_b, h, s, &sim));
        store.tracker().force_complete(s);
    }
    {
        let mut slots = [CellSlot { id: id_a, cell: &mut cell_a }, CellSlot { id: id_b, cell: &mut cell_b }];
        // Last frame before device loss: next-frame witness dropped.
        scene_boundary(&mut frames, sim, &mut store, &mut slots);
    }

    let base_a_before = store.row_region_base(id_a) as usize;
    let base_b_before = store.row_region_base(id_b) as usize;
    // Slot region bases: no public accessor exists, so derive them from the
    // deterministic first-fit `RegionPool` allocation order — cell A
    // registered first gets slot base 0, cell B (registered second, same
    // class) gets the next region at `slot_region_size`.
    let slot_base_a_before = 0usize;
    let slot_base_b_before = slot_region_size as usize;

    let before_rows = readback(&ctx1, store.transform_buffer(), transform_bytes);
    let before_mirror = readback(&ctx1, store.slot_mirror_buffer(), mirror_bytes);
    let before_gens = readback(&ctx1, store.generation_buffer(), gen_bytes);
    let before_infos = readback(&ctx1, store.instance_info_buffer(), info_bytes);

    // Device loss: drop the store, then the entire device.
    drop(store);
    drop(ctx1);

    // Fresh device; rebuild both cells from CPU-authoritative columns only.
    let ctx2 = test_context();
    let (rebuilt, ids) = SceneGpuStore::rebuild(&ctx2, cfg, &[(0, &cell_a), (0, &cell_b)]);
    let id2_a = ids[0];
    let id2_b = ids[1];
    let base_a_after = rebuilt.row_region_base(id2_a) as usize;
    let base_b_after = rebuilt.row_region_base(id2_b) as usize;
    // Same deterministic first-fit order as above — cell A first, cell B
    // second — so the slot bases in the rebuilt store match the pre-loss
    // ones. Kept as separate named values (not reused) to make the
    // region-relative comparison below self-documenting.
    let slot_base_a_after = 0usize;
    let slot_base_b_after = slot_region_size as usize;

    let after_rows = readback(&ctx2, rebuilt.transform_buffer(), transform_bytes);
    let after_mirror = readback(&ctx2, rebuilt.slot_mirror_buffer(), mirror_bytes);
    let after_gens = readback(&ctx2, rebuilt.generation_buffer(), gen_bytes);
    let after_infos = readback(&ctx2, rebuilt.instance_info_buffer(), info_bytes);

    // Cell A: byte-identity over its region-relative slices.
    let rows_a = cell_a.rows_in_use() as usize;
    let rows_bytes_a = rows_a * 64;
    assert_eq!(
        after_rows[base_a_after * 64..base_a_after * 64 + rows_bytes_a],
        before_rows[base_a_before * 64..base_a_before * 64 + rows_bytes_a],
        "cell A transforms byte-identical across device loss"
    );
    let mirror_bytes_a = rows_a * 4;
    assert_eq!(
        after_mirror[base_a_after * 4..base_a_after * 4 + mirror_bytes_a],
        before_mirror[base_a_before * 4..base_a_before * 4 + mirror_bytes_a],
        "cell A slot mirror byte-identical across device loss"
    );
    let gens_bytes_a = cell_a.registry().generations().len() * 4;
    assert_eq!(
        after_gens[slot_base_a_after * 4..slot_base_a_after * 4 + gens_bytes_a],
        before_gens[slot_base_a_before * 4..slot_base_a_before * 4 + gens_bytes_a],
        "cell A generations byte-identical across device loss"
    );
    let info_bytes_a = rows_a * 8;
    assert_eq!(
        after_infos[base_a_after * 8..base_a_after * 8 + info_bytes_a],
        before_infos[base_a_before * 8..base_a_before * 8 + info_bytes_a],
        "cell A instance-info region byte-identical across device loss"
    );

    // Cell B: same, at its own (non-zero) region bases.
    let rows_b = cell_b.rows_in_use() as usize;
    let rows_bytes_b = rows_b * 64;
    assert_eq!(
        after_rows[base_b_after * 64..base_b_after * 64 + rows_bytes_b],
        before_rows[base_b_before * 64..base_b_before * 64 + rows_bytes_b],
        "cell B transforms byte-identical across device loss"
    );
    let mirror_bytes_b = rows_b * 4;
    assert_eq!(
        after_mirror[base_b_after * 4..base_b_after * 4 + mirror_bytes_b],
        before_mirror[base_b_before * 4..base_b_before * 4 + mirror_bytes_b],
        "cell B slot mirror byte-identical across device loss"
    );
    let gens_bytes_b = cell_b.registry().generations().len() * 4;
    assert_eq!(
        after_gens[slot_base_b_after * 4..slot_base_b_after * 4 + gens_bytes_b],
        before_gens[slot_base_b_before * 4..slot_base_b_before * 4 + gens_bytes_b],
        "cell B generations byte-identical across device loss"
    );
    let info_bytes_b = rows_b * 8;
    assert_eq!(
        after_infos[base_b_after * 8..base_b_after * 8 + info_bytes_b],
        before_infos[base_b_before * 8..base_b_before * 8 + info_bytes_b],
        "cell B instance-info region byte-identical across device loss"
    );
}

/// M2b-β Task 1: a `SpatialCell::with_transform` cell — carrying both the six
/// bounds columns AND a token-registered `[f32; 16]` transform column — is
/// accepted end-to-end by `gpu::SceneGpuStore`, which resolves the mirrored
/// column token-keyed (`column_for_mut::<[f32;16]>`). Proves
/// `register_token_column` wires the token index correctly for a
/// positionally-constructed cell.
/// M2b-β Task 4 gate 1: a region evicted via `unregister_cell` is pinned by
/// `last_serial` exactly like the M2a row/slot pin-by-serial pattern —
/// unusable for a fresh registration until that serial's frame boundary
/// drains it (`retire_all` now drains both pool families every boundary).
#[test]
fn eviction_returns_region_only_after_serial_completes() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
            tombstone_headroom: 8,
            max_cells_metadata: 4,
        },
    );
    let mut frames = FrameDriver::new();
    let mut cell_a = transform_cell(64);
    let id_a = store.register_cell(&cell_a, 0).unwrap();
    let serial = store.tracker().next_serial();
    store.unregister_cell(id_a, &mut cell_a, serial);
    // Region still pinned: a new registration must fail.
    let cell_b = transform_cell(64);
    assert!(store.register_cell(&cell_b, 0).is_err(), "region pinned until serial completes");
    // Complete the serial; the drain happens in retire (frame boundary):
    store.tracker().force_complete(serial);
    let sim = frames.begin();
    let b = sim.end().end().end();
    let (retired, _) = b.retire(&mut store, &mut []);
    let compacted = retired.compact(&mut store, &mut []);
    compacted.sync(&mut store, &mut []);
    assert!(store.register_cell(&cell_b, 0).is_ok(), "region recycled after drain");
}

/// M2b-β Task 4 gate 2 (D2 eviction-timing refinement): a pending retire
/// still queued when its cell is evicted commits CPU-side IMMEDIATELY —
/// zero VRAM writes, since the region is about to be discarded wholesale —
/// rather than waiting for the eviction serial to complete.
#[test]
fn eviction_commits_pending_retires_cpu_side_with_zero_vram_writes() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut frames = FrameDriver::new();
    let mut cell = transform_cell(64);
    let id = store.register_cell(&cell, 0).unwrap();
    let h = cell.alloc().unwrap();
    let sim = frames.begin();
    store.write_transform(id, &mut cell, h, &mat(1.0), &sim);
    let b = sim.end().end().end();
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        b.run(&mut store, &mut slots);
    }
    // Deferred-free h, then evict BEFORE its serial completes:
    let sim = frames.begin();
    let s = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, h, s, &sim);
    drop(sim);
    let writes_before = store.generation_write_count();
    store.unregister_cell(id, &mut cell, s);
    assert_eq!(store.generation_write_count(), writes_before, "zero VRAM writes at eviction");
    // CPU-side: handle stale, slot recycled, row unpinned+compactable:
    assert_eq!(cell.row_of(h), None, "pending retire committed CPU-side");
    let h2 = cell.alloc().unwrap();
    assert_eq!(h2.index(), h.index(), "slot recycled");
}

/// M2b-β Task 4 gate 3 (D2-tail carry-forward, §11): a region recycled from
/// an evicted tenant must never expose that tenant's stale generations in
/// the tail beyond the new tenant's occupied-slot prefix — `register_cell`'s
/// tail scrub zero-fills `[gens.len()..slot_capacity)` in both VRAM and the
/// gen-shadow on every promotion, fresh region or recycled.
#[test]
fn d2_tail_recycled_region_never_exposes_prior_generations() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 1 }],
            tombstone_headroom: 8,
            max_cells_metadata: 4,
        },
    );
    let mut frames = FrameDriver::new();
    // Tenant A: churn slot 0 through several generations so the region holds
    // non-zero generations across more than one slot.
    let mut cell_a = transform_cell(64);
    for _ in 0..3 {
        let h = cell_a.alloc().unwrap();
        cell_a.free(h);
    }
    for _ in 0..3 {
        cell_a.alloc().unwrap();
    }
    let id_a = store.register_cell(&cell_a, 0).unwrap();
    let serial = store.tracker().next_serial();
    store.unregister_cell(id_a, &mut cell_a, serial);
    store.tracker().force_complete(serial);
    {
        let sim = frames.begin();
        let b = sim.end().end().end();
        let (r, _) = b.retire(&mut store, &mut []);
        r.compact(&mut store, &mut []).sync(&mut store, &mut []);
    }
    // Tenant B: ONE slot only — the region tail must not show A's residual
    // generations.
    let mut cell_b = transform_cell(64);
    cell_b.alloc().unwrap();
    let _id_b = store.register_cell(&cell_b, 0).unwrap();
    let gens = as_u32s(&readback(&ctx, store.generation_buffer(), (72 * 4) as u64));
    assert_eq!(gens[0], 1, "B's slot 0");
    assert!(
        gens[1..72].iter().all(|&g| g == 0),
        "tail scrubbed — no prior-tenant generations survive (found {:?})",
        gens[1..72].iter().enumerate().filter(|(_, &g)| g != 0).take(4).collect::<Vec<_>>()
    );
}

/// M2b-β Task 5: the boundary transition executor wired end-to-end — an
/// observer inside cell (0,0) queues an Outer→Margin `Transition` via
/// `classify`; `execute_transitions` (run after this frame's `retire` stage,
/// per its `&RetiredPhase` witness) drains it, calls `register_cell`, and
/// records the resulting `CellId` on the grid (`gpu_id`). After
/// `advance_crossfade`, `write_cell_metadata` packs the cell's α/domain pair
/// at its dense id's 8-byte slot in the cell-metadata SSBO, read back and
/// checked byte-for-byte.
#[test]
fn transitions_execute_at_boundary_and_metadata_mirrors_state() {
    use pulsar_scenedb::gpu::{execute_transitions, CellCoord, Domain, GridConfig, StreamingBudget, StreamingGrid};
    use std::collections::HashMap;

    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut frames = FrameDriver::new();
    let mut grid = StreamingGrid::new(
        GridConfig { cell_width: 100.0, margin_radius: 150.0, pad_fraction: 0.10, hysteresis: 20.0 },
        StreamingBudget {
            vram_hlod_budget: u64::MAX,
            vram_geometry_budget: u64::MAX,
            max_materialized_cells: 16,
            proxy_mesh_bytes: 1,
            mean_cell_geometry_bytes: 1,
        },
        &[RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
    )
    .unwrap();
    let c0 = CellCoord { x: 0, z: 0 };
    grid.materialize(c0);
    let mut cells = HashMap::new();
    cells.insert(c0, pulsar_scenedb::SpatialCell::with_transform(64).unwrap());

    // Observer inside cell 0 → Outer→Margin queued:
    grid.classify(&[pulsar_scenedb::Aabb { min: [40.0, -1.0, -1.0], max: [60.0, 1.0, 1.0] }]);

    let sim = frames.begin();
    let b = sim.end().end().end();
    let (retired, _) = b.retire(&mut store, &mut []);
    let serial = store.tracker().next_serial();
    let stats = execute_transitions(&mut grid, &mut store, &mut cells, &|_| 0, serial, &retired);
    assert_eq!(stats.promoted, 1);
    assert_eq!(stats.demoted, 0);
    assert_eq!(stats.declined, 0);
    assert_eq!(stats.dropped_stale, 0);
    assert_eq!(grid.domain(c0), Some(Domain::Margin));
    assert!(grid.gpu_id(c0).is_some(), "resident cell has a region");

    retired.compact(&mut store, &mut []).sync(&mut store, &mut []);
    grid.advance_crossfade(50.0, 100.0);
    // Test 13 instrumentation (M3-α T7): `write_cell_metadata` takes `&self`
    // (see its `AtomicU64` counter), so the assert below proves it moves on
    // every call — including this one — with no rejection path to test
    // here (the method has no validation branch, only a hard capacity
    // assert).
    assert_eq!(grid.upload_count(), 0, "no cell-metadata upload before the first write_cell_metadata call");
    grid.write_cell_metadata(ctx.queue(), store.cell_metadata_buffer());
    assert_eq!(grid.upload_count(), 1, "write_cell_metadata counted its upload");

    let meta = readback(&ctx, store.cell_metadata_buffer(), 8);
    let alpha = f32::from_le_bytes(meta[0..4].try_into().unwrap());
    let domain = u32::from_le_bytes(meta[4..8].try_into().unwrap());
    assert!((alpha - 0.5).abs() < 1e-6);
    assert_eq!(domain, 1, "Margin encodes as 1");
}

#[test]
fn spatial_cell_with_transform_registers_and_syncs() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    let mut frames = FrameDriver::new();
    let mut sc = pulsar_scenedb::SpatialCell::with_transform(64).unwrap();
    let id = store.register_cell(sc.storage(), 0).unwrap();
    let h = sc.alloc(pulsar_scenedb::Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    let sim = frames.begin();
    assert!(store.write_transform(id, sc.storage_mut(), h, &mat(5.0), &sim));
    let b = sim.end().end().end();
    let mut slots = [CellSlot { id, cell: sc.storage_mut() }];
    b.run(&mut store, &mut slots);
    let base = store.row_region_base(id) as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), (64 * 4 * 64) as u64));
    assert_eq!(&gpu[base * 16..base * 16 + 16], &mat(5.0));
}
