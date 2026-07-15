//! GeometryArena headless verification (design Rev 2 §3): real surfaceless
//! wgpu device; the test harness owns the `device.poll` pump.
//!
//! `test_context`/`readback` are copied verbatim from `tests/gpu_store.rs` —
//! integration test binaries cannot share modules without a common
//! `tests/common/mod.rs`, and that refactor is deliberately out of scope here.

use pulsar_scenedb::gpu::{ArenaError, ClusterBuffer, ClusterError, ClusterNode, EngineGpuContext, GeometryArena, MeshError, MeshMetadata, MeshRegistry};
use std::sync::Arc;

/// Byte view of `MeshMetadata` entries for readback comparison. Mirrors the
/// crate-internal `gpu::as_bytes` (pub(crate) — not visible to this
/// integration test binary, which only sees the crate's public API).
///
/// SAFETY: `MeshMetadata` is `#[repr(C)]`, `Copy`, and the crate's own
/// `const _: () = assert!(size_of::<MeshMetadata>() == 72)` pins its layout
/// to exactly 72 bytes with no padding.
fn mesh_bytes(entries: &[MeshMetadata]) -> Vec<u8> {
    unsafe {
        std::slice::from_raw_parts(entries.as_ptr() as *const u8, std::mem::size_of_val(entries))
    }
    .to_vec()
}

fn traditional_mesh() -> MeshMetadata {
    MeshMetadata {
        vertex_offset: 64,
        index_offset: 128,
        index_count: 300,
        base_vertex: -7,
        material_index: 3,
        lod_count: 2,
        lod_distances: [10.0, 20.0, 0.0, 0.0],
        local_aabb_center: [1.0, 2.0, 3.0],
        cluster_table_offset: 0,
        local_aabb_extents: [0.5, 0.5, 0.5],
        meshlet_count: 0,
    }
}

fn vg_mesh() -> MeshMetadata {
    MeshMetadata {
        vertex_offset: 256,
        index_offset: 512,
        index_count: 900,
        base_vertex: 0,
        material_index: 5,
        lod_count: 0,
        lod_distances: [0.0, 0.0, 0.0, 0.0],
        local_aabb_center: [-1.0, -2.0, -3.0],
        cluster_table_offset: 100,
        local_aabb_extents: [4.0, 4.0, 4.0],
        meshlet_count: 42,
    }
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

#[test]
fn traditional_and_vg_mesh_registered_byte_exact_in_ssbo() {
    let ctx = test_context();
    let mut reg = MeshRegistry::new(&ctx, 4);

    let traditional = traditional_mesh();
    let vg = vg_mesh();
    let idx_a = reg.register(ctx.queue(), traditional).expect("traditional mesh (XOR satisfied by lod_count)");
    let idx_b = reg.register(ctx.queue(), vg).expect("VG mesh (XOR satisfied by cluster_table_offset)");
    assert_eq!((idx_a, idx_b), (0, 1));
    assert_eq!(reg.len(), 2);
    assert_eq!(reg.get(idx_a), &traditional);
    assert_eq!(reg.get(idx_b), &vg);

    let gpu = readback(&ctx, reg.buffer(), 2 * 72);
    let expected = mesh_bytes(reg.entries());
    assert_eq!(expected.len(), 144, "two 72-byte records");
    assert_eq!(gpu, expected, "SSBO bytes must exactly mirror as_bytes(entries())");
}

#[test]
fn register_rejects_both_fields_non_zero() {
    let ctx = test_context();
    let mut reg = MeshRegistry::new(&ctx, 4);
    let mut m = traditional_mesh();
    m.cluster_table_offset = 100; // now BOTH lod_count and cluster_table_offset are non-zero
    let err = reg.register(ctx.queue(), m);
    assert_eq!(err, Err(MeshError::XorRule));
    assert_eq!(reg.len(), 0, "rejected registration must not partially land");
}

#[test]
fn register_rejects_both_fields_zero() {
    let ctx = test_context();
    let mut reg = MeshRegistry::new(&ctx, 4);
    let mut m = traditional_mesh();
    m.lod_count = 0; // now BOTH lod_count and cluster_table_offset are zero
    let err = reg.register(ctx.queue(), m);
    assert_eq!(err, Err(MeshError::XorRule));
    assert_eq!(reg.len(), 0);
}

#[test]
fn register_fails_hard_once_registry_is_full() {
    let ctx = test_context();
    let mut reg = MeshRegistry::new(&ctx, 2);
    assert!(reg.register(ctx.queue(), traditional_mesh()).is_ok());
    assert!(reg.register(ctx.queue(), vg_mesh()).is_ok());
    let err = reg.register(ctx.queue(), traditional_mesh());
    assert_eq!(err, Err(MeshError::RegistryFull));
    assert_eq!(reg.len(), 2, "full registry rejects without growing");
}

/// Byte view of `ClusterNode` entries for readback comparison. Mirrors the
/// crate-internal `gpu::as_bytes` (pub(crate) — not visible to this
/// integration test binary, which only sees the crate's public API).
///
/// SAFETY: `ClusterNode` is `#[repr(C)]`, `Copy`, and the crate's own
/// `const _: () = assert!(size_of::<ClusterNode>() == 48)` pins its layout
/// to exactly 48 bytes with no padding.
fn cluster_bytes(nodes: &[ClusterNode]) -> Vec<u8> {
    unsafe {
        std::slice::from_raw_parts(nodes.as_ptr() as *const u8, std::mem::size_of_val(nodes))
    }
    .to_vec()
}

fn test_cluster_node() -> ClusterNode {
    ClusterNode {
        meshlet_offset: 0,
        meshlet_count: 5,
        parent_error: 1.0,
        self_error: 0.5,
        group_id: 7,
        child_offset: 10,
        child_count: 3,
        padding: 0,
        bounding_sphere: [1.0, 2.0, 3.0, 0.5],
    }
}

#[test]
fn append_two_valid_nodes_returns_correct_offsets() {
    let ctx = test_context();
    let mut cluster = ClusterBuffer::new(&ctx, 4);

    let node1 = test_cluster_node();
    let node2 = ClusterNode {
        meshlet_offset: 5,
        meshlet_count: 3,
        parent_error: 2.0,
        self_error: 1.0,
        group_id: 8,
        child_offset: 13,
        child_count: 2,
        padding: 0,
        bounding_sphere: [0.0, 0.0, 0.0, 1.0],
    };

    let offset1 = cluster.append(ctx.queue(), &[node1]).expect("first append");
    let offset2 = cluster.append(ctx.queue(), &[node2]).expect("second append");

    assert_eq!(offset1, 0, "first append returns offset 0");
    assert_eq!(offset2, 1, "second append returns offset 1");
    assert_eq!(cluster.len(), 2);
    assert_eq!(cluster.get(0), &node1);
    assert_eq!(cluster.get(1), &node2);
}

#[test]
fn cluster_nodes_readback_byte_exact() {
    let ctx = test_context();
    let mut cluster = ClusterBuffer::new(&ctx, 4);

    let node1 = test_cluster_node();
    let node2 = ClusterNode {
        meshlet_offset: 5,
        meshlet_count: 3,
        parent_error: 2.0,
        self_error: 1.0,
        group_id: 8,
        child_offset: 13,
        child_count: 2,
        padding: 0,
        bounding_sphere: [0.0, 0.0, 0.0, 1.0],
    };

    cluster.append(ctx.queue(), &[node1]).expect("first append");
    cluster.append(ctx.queue(), &[node2]).expect("second append");

    let gpu = readback(&ctx, cluster.buffer(), 2 * 48);
    let expected = cluster_bytes(cluster.nodes());
    assert_eq!(expected.len(), 96, "two 48-byte records");
    assert_eq!(gpu, expected, "SSBO bytes must exactly mirror as_bytes(nodes())");
}

#[test]
fn append_rejects_error_monotonicity_violation() {
    let ctx = test_context();
    let mut cluster = ClusterBuffer::new(&ctx, 4);

    let bad_node = ClusterNode {
        meshlet_offset: 0,
        meshlet_count: 1,
        parent_error: 0.5,
        self_error: 1.0,
        padding: 0,
        group_id: 0,
        child_offset: 0,
        child_count: 0,
        bounding_sphere: [0.0, 0.0, 0.0, 1.0],
    };

    let err = cluster.append(ctx.queue(), &[bad_node]);
    assert_eq!(err, Err(ClusterError::ErrorMonotonicity));
    assert_eq!(cluster.len(), 0, "rejected batch must not consume offsets");
}

#[test]
fn append_rejects_padding_nonzero() {
    let ctx = test_context();
    let mut cluster = ClusterBuffer::new(&ctx, 4);

    let bad_node = ClusterNode {
        meshlet_offset: 0,
        meshlet_count: 1,
        parent_error: 1.0,
        self_error: 0.5,
        group_id: 0,
        child_offset: 0,
        child_count: 0,
        padding: 1,
        bounding_sphere: [0.0, 0.0, 0.0, 1.0],
    };

    let err = cluster.append(ctx.queue(), &[bad_node]);
    assert_eq!(err, Err(ClusterError::PaddingNonZero));
    assert_eq!(cluster.len(), 0, "rejected batch must not consume offsets");
}

#[test]
fn append_fails_when_buffer_full() {
    let ctx = test_context();
    let mut cluster = ClusterBuffer::new(&ctx, 2);

    let node = test_cluster_node();

    assert!(cluster.append(ctx.queue(), &[node]).is_ok());
    assert!(cluster.append(ctx.queue(), &[node]).is_ok());
    let err = cluster.append(ctx.queue(), &[node]);
    assert_eq!(err, Err(ClusterError::BufferFull));
    assert_eq!(cluster.len(), 2, "full buffer rejects without growing");
}
