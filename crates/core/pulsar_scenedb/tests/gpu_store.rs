//! M2a headless verification (design Rev 3 §9): real surfaceless wgpu device;
//! the test harness owns the `device.poll` pump.

use pulsar_scenedb::gpu::EngineGpuContext;
use pulsar_scenedb::gpu::SceneBuffer;
use pulsar_scenedb::gpu::DirtyMask;
use pulsar_scenedb::gpu::{CellSlot, FrameDriver, RegionClassConfig, SceneGpuConfig, SceneGpuStore, SimulateA};
use pulsar_scenedb::{CellStorage, CellType, TypeToken};
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

fn scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig { capacity: 64, max_resident_cells: 4 }],
        tombstone_headroom: 8,
        max_materials: 16,
        max_cells_metadata: 16,
    }
}

/// Drives one full frame boundary through the compile-time phase machine
/// (T11, design Rev 2 §6) — the only path available to callers outside this
/// crate now that `retire_all`/`compact_all`/`sync_all` are `pub(crate)`.
/// Consumes the current `SimulateA` witness through the full
/// Simulate→Harvest→Boundary chain and leaves `*sim` holding a fresh
/// `SimulateA` (from a new `FrameDriver::begin`) for the next frame's
/// mutations.
fn scene_boundary(
    frames: &mut FrameDriver,
    sim: &mut SimulateA,
    store: &mut SceneGpuStore,
    slots: &mut [CellSlot<'_>],
) -> pulsar_scenedb::gpu::SyncStats {
    let cur = std::mem::replace(sim, frames.begin());
    cur.end().end().end().run(store, slots)
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
    }
    // Free hb via the deferred path; complete its serial; boundary again:
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(id, &mut cell, hb, serial, &sim));
    store.tracker().force_complete(serial);
    let stats = {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots) // retire → compact (hc moves) → sync
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
    let cur = std::mem::replace(&mut sim, frames.begin());
    let retired = cur.end().end().end().retire(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
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
/// `retire_all`'s drained-count is no longer directly observable outside the
/// crate (it is `pub(crate)`, T11) — the between-stage asserts that used to
/// read it now go through `RetiredPhase`/`CompactedPhase` (reachable via
/// `BoundaryPhase::retire`/`RetiredPhase::compact`) and confirm the SAME fact
/// via `generation_write_count()`: retirement always bumps a slot's
/// generation, so the shadow-gated write count rises by exactly the number of
/// entries drained.
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
    }

    let row = cell.row_of(h).unwrap();
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(id, &mut cell, h, serial, &sim));

    // Serial INCOMPLETE: boundary runs but nothing retires.
    let gens_before = store.generation_write_count();
    let cur = std::mem::replace(&mut sim, frames.begin());
    let b = cur.end().end().end();
    let retired = b.retire(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
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
    let cur = std::mem::replace(&mut sim, frames.begin());
    let b = cur.end().end().end();
    let retired = b.retire(&mut store, &mut [CellSlot { id, cell: &mut cell }]);
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
    }
    for &h in &[hs[2], hs[5]] {
        let s = store.tracker().next_serial();
        store.free_deferred(id, &mut cell, h, s, &sim);
        store.tracker().force_complete(s);
    }
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
    let mut sim = frames.begin();
    let ha = cell_a.alloc().unwrap();
    let hb = cell_b.alloc().unwrap();
    assert!(store.write_transform(ida, &mut cell_a, ha, &mat(1.0), &sim));
    assert!(store.write_transform(idb, &mut cell_b, hb, &mat(2.0), &sim));
    {
        let mut slots = [CellSlot { id: ida, cell: &mut cell_a }, CellSlot { id: idb, cell: &mut cell_b }];
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
            max_materials: 1,
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
    }
    // Retire ha; hc swaps into ha's row (row 0); boundary uploads the move
    // and stamps ha's bumped generation into the gen-shadow.
    let serial = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, ha, serial, &sim);
    store.tracker().force_complete(serial);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
    }
    // Retire ha; hc swaps into row0; rows_in_use shrinks to 2 — row2 is
    // vacated but mirror[row2] still holds hc's slot (stale-but-inert while
    // unoccupied).
    let serial = store.tracker().next_serial();
    store.free_deferred(id, &mut cell, ha, serial, &sim);
    store.tracker().force_complete(serial);
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
    }
    // Re-occupy row2 with a recycled slot and DO NOT write its transform:
    // no write-path trigger can ever fire for this row.
    let hd = cell.alloc().unwrap();
    let hd_row = cell.row_of(hd).unwrap() as usize;
    assert_eq!(hd_row, 2, "precondition: hd re-occupied the vacated tail row");
    assert_ne!(hd.index(), hc.index(), "precondition: hd's slot differs from the stale mirror entry (non-vacuous)");
    {
        let mut slots = [CellSlot { id, cell: &mut cell }];
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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

    let mut cell_a = transform_cell(64);
    let mut cell_b = transform_cell(64);

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
    }
    let hs_b: Vec<_> = (0..8).map(|_| cell_b.alloc().unwrap()).collect();
    for (i, &h) in hs_b.iter().enumerate() {
        assert!(store.write_transform(id_b, &mut cell_b, h, &mat(1000.0 + i as f32 * 10.0), &sim));
    }
    {
        let mut slots = [CellSlot { id: id_a, cell: &mut cell_a }, CellSlot { id: id_b, cell: &mut cell_b }];
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
        scene_boundary(&mut frames, &mut sim, &mut store, &mut slots);
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
}
