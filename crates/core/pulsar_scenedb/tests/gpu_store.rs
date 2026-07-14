//! M2a headless verification (design Rev 3 §9): real surfaceless wgpu device;
//! the test harness owns the `device.poll` pump.

use pulsar_scenedb::gpu::EngineGpuContext;
use pulsar_scenedb::gpu::SceneBuffer;
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
