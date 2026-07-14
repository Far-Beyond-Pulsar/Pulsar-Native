//! M2a headless verification (design Rev 3 §9): real surfaceless wgpu device;
//! the test harness owns the `device.poll` pump.

use pulsar_scenedb::gpu::EngineGpuContext;
use pulsar_scenedb::gpu::SceneBuffer;
use pulsar_scenedb::gpu::{GpuStore, GpuStoreConfig};
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
    let cpu: Vec<[f32; 16]> = (0..4).map(|i| mat(i as f32 * 100.0)).collect();
    for row in 0..4 {
        buf.mark_row_dirty(row);
    }
    let stats = buf.sync(ctx.queue(), &cpu);
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
    let cpu: Vec<[f32; 16]> = (0..64).map(|i| mat(i as f32)).collect();
    // Warm upload.
    for row in 0..64 {
        buf.mark_row_dirty(row);
    }
    buf.sync(ctx.queue(), &cpu);
    // Zero-mutation frame writes nothing.
    let stats = buf.sync(ctx.queue(), &cpu);
    assert_eq!((stats.ranges, stats.bytes), (0, 0), "clean frame is free");
    // Scattered dirty rows: {3}, {10,11,12}, {60} → exactly 3 ranges.
    for row in [3u32, 10, 11, 12, 60] {
        buf.mark_row_dirty(row);
    }
    let stats = buf.sync(ctx.queue(), &cpu);
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

fn store_and_cell(ctx: &EngineGpuContext) -> (GpuStore, CellStorage) {
    (
        GpuStore::new(ctx, GpuStoreConfig { max_rows: 64, max_slots: 64 }),
        transform_cell(64),
    )
}

/// Run one frame boundary: retire → compact → sync.
fn frame_boundary(store: &mut GpuStore, cell: &mut CellStorage) -> pulsar_scenedb::gpu::SyncStats {
    store.retire(cell);
    store.compact(cell);
    store.sync(cell)
}

#[test]
fn write_transform_is_the_single_mutation_path() {
    let ctx = test_context();
    let (mut store, mut cell) = store_and_cell(&ctx);
    let h = cell.alloc().unwrap();
    assert!(store.write_transform(&mut cell, h, &mat(9.0)));
    frame_boundary(&mut store, &mut cell);
    let row = cell.row_of(h).unwrap() as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), 64 * 64));
    assert_eq!(&gpu[row * 16..row * 16 + 16], &mat(9.0));
    // Stale handle rejected.
    let dead = cell.alloc().unwrap();
    cell.free(dead);
    assert!(!store.write_transform(&mut cell, dead, &mat(0.0)));
}

#[test]
fn compaction_move_is_resynced_and_generation_buffer_matches_registry() {
    let ctx = test_context();
    let (mut store, mut cell) = store_and_cell(&ctx);
    let ha = cell.alloc().unwrap();
    let hb = cell.alloc().unwrap();
    let hc = cell.alloc().unwrap();
    for (h, s) in [(ha, 1.0f32), (hb, 2.0), (hc, 3.0)] {
        store.write_transform(&mut cell, h, &mat(s));
    }
    frame_boundary(&mut store, &mut cell);
    // Free hb via the deferred path; complete its serial; boundary again:
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(&mut cell, hb, serial));
    store.tracker().force_complete(serial);
    let stats = frame_boundary(&mut store, &mut cell); // retire → compact (hc moves) → sync
    assert!(stats.ranges >= 1, "the compaction move was re-uploaded");
    // Moved row's GPU bytes are correct at its NEW index:
    let hc_row = cell.row_of(hc).unwrap() as usize;
    let gpu = as_f32s(&readback(&ctx, store.transform_buffer(), 64 * 64));
    assert_eq!(&gpu[hc_row * 16..hc_row * 16 + 16], &mat(3.0));
    // Generation buffer matches the registry for every allocated slot:
    let regs = cell.registry().generations().to_vec();
    let gpu_gens = as_u32s(&readback(&ctx, store.generation_buffer(), 64 * 4));
    assert_eq!(&gpu_gens[..regs.len()], &regs[..]);
}

#[test]
fn generation_uploads_are_shadow_gated_to_changes_only() {
    let ctx = test_context();
    let (mut store, mut cell) = store_and_cell(&ctx);
    let h = cell.alloc().unwrap();
    // Same write window: two transform writes, one generation upload.
    assert!(store.write_transform(&mut cell, h, &mat(1.0)));
    assert!(store.write_transform(&mut cell, h, &mat(2.0)));
    assert_eq!(
        store.generation_write_count(),
        1,
        "repeat writes to a live handle upload its generation exactly once"
    );
    // Next frame: a moving object's write is still generation-silent.
    frame_boundary(&mut store, &mut cell);
    assert!(store.write_transform(&mut cell, h, &mat(3.0)));
    assert_eq!(
        store.generation_write_count(),
        1,
        "unchanged generation is never re-uploaded across frames"
    );
    // Retirement bumps the generation → exactly one more upload.
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(&mut cell, h, serial));
    store.tracker().force_complete(serial);
    store.retire(&mut cell);
    assert_eq!(store.generation_write_count(), 2, "retirement writes the bumped generation");
    // Close the frame boundary (phase machine: retire → compact → sync).
    store.compact(&mut cell);
    store.sync(&cell);
}

/// Test 6 host-side (design §9): the retirement invariant. A slot is never
/// reissued, and its row never reclaimed, before its serial completes and the
/// new generation is in the VRAM buffer; the handle stays row-resolvable but
/// harvest-dead during the window; afterwards it is rejected. No UB.
#[test]
fn test6_retirement_invariant() {
    let ctx = test_context();
    let (mut store, mut cell) = store_and_cell(&ctx);
    let h = cell.alloc().unwrap();
    store.write_transform(&mut cell, h, &mat(42.0));
    frame_boundary(&mut store, &mut cell);

    let row = cell.row_of(h).unwrap();
    let serial = store.tracker().next_serial();
    assert!(store.free_deferred(&mut cell, h, serial));

    // Serial INCOMPLETE: boundary runs but nothing retires.
    assert_eq!(store.retire(&mut cell), 0, "incomplete serial must not retire");
    store.compact(&mut cell);
    assert_eq!(cell.row_of(h), Some(row), "row not compacted while pinned");
    store.sync(&cell);
    let h2 = cell.alloc().unwrap();
    assert_ne!(h2.index(), h.index(), "slot not reissued while in flight");
    assert_eq!(cell.live_count(), 1, "pending row absent from harvest (only h2 lives)");

    // Serial COMPLETES: the drain writes VRAM gen BEFORE pooling the slot.
    store.tracker().force_complete(serial);
    assert_eq!(store.retire(&mut cell), 1);
    let gpu_gens = as_u32s(&readback(&ctx, store.generation_buffer(), 64 * 4));
    assert_eq!(gpu_gens[h.index() as usize], h.generation() + 1, "VRAM generation bumped");
    store.compact(&mut cell);
    store.sync(&cell);
    assert_eq!(cell.row_of(h), None, "old handle rejected after retirement");
    let h3 = cell.alloc().unwrap();
    assert_eq!(h3.index(), h.index(), "slot recycled only now");
    assert_eq!(h3.generation(), h.generation() + 1);
}
