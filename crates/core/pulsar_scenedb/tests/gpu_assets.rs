//! GeometryArena headless verification (design Rev 2 §3): real surfaceless
//! wgpu device; the test harness owns the `device.poll` pump.
//!
//! `test_context`/`readback` are copied verbatim from `tests/gpu_store.rs` —
//! integration test binaries cannot share modules without a common
//! `tests/common/mod.rs`, and that refactor is deliberately out of scope here.

use pulsar_scenedb::gpu::{ArenaError, EngineGpuContext, GeometryArena};
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
fn upload_two_vertex_blobs_are_disjoint_and_byte_exact() {
    let ctx = test_context();
    let mut arena = GeometryArena::new(&ctx, 1024, 1024);
    let blob_a: Vec<u8> = (0..64u8).collect();
    let blob_b: Vec<u8> = (100..164u8).collect();
    let off_a = arena.upload_vertices(ctx.queue(), &blob_a).unwrap();
    let off_b = arena.upload_vertices(ctx.queue(), &blob_b).unwrap();
    assert_ne!(off_a, off_b, "disjoint offsets");
    assert_eq!(off_a, 0);
    assert_eq!(off_b, 64);

    let gpu = readback(&ctx, arena.vertex_buffer(), 128);
    assert_eq!(&gpu[off_a as usize..off_a as usize + blob_a.len()], &blob_a[..], "blob A byte-exact");
    assert_eq!(&gpu[off_b as usize..off_b as usize + blob_b.len()], &blob_b[..], "blob B byte-exact");
}

#[test]
fn tiny_arena_exhaustion_is_a_hard_error() {
    let ctx = test_context();
    let mut arena = GeometryArena::new(&ctx, 16, 16);
    // First alloc consumes the whole vertex arena.
    assert!(arena.upload_vertices(ctx.queue(), &[1u8; 16]).is_ok());
    // Second alloc has nowhere to go.
    let err = arena.upload_vertices(ctx.queue(), &[2u8; 1]);
    assert_eq!(err, Err(ArenaError::Exhausted));

    // Same for the index arena.
    assert!(arena.upload_indices(ctx.queue(), &[1u8; 16]).is_ok());
    let err = arena.upload_indices(ctx.queue(), &[2u8; 1]);
    assert_eq!(err, Err(ArenaError::Exhausted));
}

#[test]
fn free_then_realloc_reuses_space_after_coalescing() {
    let ctx = test_context();
    let mut arena = GeometryArena::new(&ctx, 64, 64);
    let a = arena.upload_vertices(ctx.queue(), &[1u8; 32]).unwrap();
    let b = arena.upload_vertices(ctx.queue(), &[2u8; 32]).unwrap();
    assert_eq!((a, b), (0, 32));

    // Arena is now full; free both (adjacent spans coalesce into the whole
    // buffer), then realloc the full size — offset equality proves reuse.
    arena.free_vertices(a, 32);
    arena.free_vertices(b, 32);
    let c = arena.upload_vertices(ctx.queue(), &[3u8; 64]).unwrap();
    assert_eq!(c, 0, "coalesced free space reused at the original offset");

    let gpu = readback(&ctx, arena.vertex_buffer(), 64);
    assert_eq!(gpu, vec![3u8; 64]);
}
