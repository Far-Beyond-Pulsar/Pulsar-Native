//! GeometryArena headless verification (design Rev 2 §3): real surfaceless
//! wgpu device; the test harness owns the `device.poll` pump.
//!
//! `test_context`/`readback` are copied verbatim from `tests/gpu_store.rs` —
//! integration test binaries cannot share modules without a common
//! `tests/common/mod.rs`, and that refactor is deliberately out of scope here.

use pulsar_scenedb::gpu::{ArenaError, ClusterBuffer, ClusterError, ClusterNode, EngineGpuContext, GeometryArena, MeshError, MeshMetadata, MeshRegistry, MeshletBuffer, MeshletEntry, MeshletError, TextureError, TextureStore};
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

    // Offset 0 is the reserved sentinel node (I1: cluster_table_offset==0
    // means "no table" under the C5 XOR rule), so real appends start at 1.
    assert_eq!(offset1, 1, "first append returns offset 1 (offset 0 is the reserved sentinel)");
    assert_eq!(offset2, 2, "second append returns offset 2");
    assert_eq!(cluster.len(), 3, "sentinel + two appended nodes");
    assert_eq!(cluster.get(offset1), &node1);
    assert_eq!(cluster.get(offset2), &node2);
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

    // Three records: the reserved sentinel (index 0) plus the two appended
    // nodes.
    let gpu = readback(&ctx, cluster.buffer(), 3 * 48);
    let expected = cluster_bytes(cluster.nodes());
    assert_eq!(expected.len(), 144, "sentinel + two 48-byte records");
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
    assert_eq!(cluster.len(), 1, "rejected batch must not consume offsets beyond the reserved sentinel");
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
    assert_eq!(cluster.len(), 1, "rejected batch must not consume offsets beyond the reserved sentinel");
}

#[test]
fn append_rejects_nan_self_error() {
    let ctx = test_context();
    let mut cluster = ClusterBuffer::new(&ctx, 4);

    let mut bad_node = test_cluster_node();
    bad_node.self_error = f32::NAN;

    // IEEE-754: `NaN >= x` is false, so a naive `self_error >= parent_error`
    // check would silently ACCEPT this node. The `!(a < b)` form must reject.
    let err = cluster.append(ctx.queue(), &[bad_node]);
    assert_eq!(err, Err(ClusterError::ErrorMonotonicity));
    assert_eq!(cluster.len(), 1, "NaN self_error must not consume offsets beyond the reserved sentinel");
}

#[test]
fn append_rejects_nan_parent_error() {
    let ctx = test_context();
    let mut cluster = ClusterBuffer::new(&ctx, 4);

    let mut bad_node = test_cluster_node();
    bad_node.parent_error = f32::NAN;

    // `self_error < NaN` is false, so `!(a < b)` routes NaN to rejection.
    let err = cluster.append(ctx.queue(), &[bad_node]);
    assert_eq!(err, Err(ClusterError::ErrorMonotonicity));
    assert_eq!(cluster.len(), 1, "NaN parent_error must not consume offsets beyond the reserved sentinel");
}

#[test]
fn batched_appends_return_offsets_1_then_3_and_read_back_byte_exact() {
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
    let node3 = ClusterNode {
        meshlet_offset: 8,
        meshlet_count: 1,
        parent_error: 4.0,
        self_error: 2.0,
        group_id: 9,
        child_offset: 15,
        child_count: 0,
        padding: 0,
        bounding_sphere: [-1.0, -2.0, -3.0, 2.0],
    };

    // The brief's literal scenario: a 2-node batch lands at offset 1 (offset
    // 0 is the reserved sentinel, I1), then a 1-node batch lands at offset 3.
    let offset_a = cluster.append(ctx.queue(), &[node1, node2]).expect("2-node batch");
    let offset_b = cluster.append(ctx.queue(), &[node3]).expect("1-node batch");
    assert_eq!(offset_a, 1, "first batch starts at node offset 1 (offset 0 is the reserved sentinel)");
    assert_eq!(offset_b, 3, "second batch starts at node offset 3");
    assert_eq!(cluster.len(), 4, "sentinel + three appended nodes");
    assert_eq!(&cluster.nodes()[1..], [node1, node2, node3]);

    let gpu = readback(&ctx, cluster.buffer(), 4 * 48);
    let expected = cluster_bytes(cluster.nodes());
    assert_eq!(expected.len(), 192, "sentinel + three 48-byte records");
    assert_eq!(gpu, expected, "SSBO bytes must exactly mirror as_bytes(nodes())");
}

#[test]
fn append_fails_when_buffer_full() {
    let ctx = test_context();
    // +1 for the reserved sentinel node 0 (I1) — still exercises the
    // two-succeed-then-fail boundary the test name promises.
    let mut cluster = ClusterBuffer::new(&ctx, 3);

    let node = test_cluster_node();

    assert!(cluster.append(ctx.queue(), &[node]).is_ok());
    assert!(cluster.append(ctx.queue(), &[node]).is_ok());
    let err = cluster.append(ctx.queue(), &[node]);
    assert_eq!(err, Err(ClusterError::BufferFull));
    assert_eq!(cluster.len(), 3, "full buffer rejects without growing (sentinel + 2 appended)");
}

/// Test 14 extension (C0 companion, M2b-α scope): the asset half of
/// device-loss re-materialization. This test re-drives the REAL per-entry
/// load path (`GeometryArena::upload_*` + `MeshRegistry::register` +
/// `ClusterBuffer::append` from caller-retained CPU data) — the asset-system
/// recovery path. The purpose-built bulk `MeshRegistry::rebuild` /
/// `ClusterBuffer::rebuild` fast path is gated separately by
/// `rebuild_reuploads_entries_over_corrupted_buffers` below, so BOTH recovery
/// shapes are covered deliberately.
#[test]
fn test14_assets_device_loss_rematerialization() {
    let ctx1 = test_context();
    let mut arena = GeometryArena::new(&ctx1, 4096, 4096);
    // Caller-retained CPU blobs — the arena itself keeps no CPU copy.
    let blob_a: Vec<u8> = (0..64u8).collect();
    let blob_b: Vec<u8> = (100..164u8).collect();
    let index_blob: Vec<u8> = (0..48u8).map(|b| b.wrapping_mul(3)).collect();
    let off_a = arena.upload_vertices(ctx1.queue(), &blob_a).unwrap();
    let off_b = arena.upload_vertices(ctx1.queue(), &blob_b).unwrap();
    let ioff = arena.upload_indices(ctx1.queue(), &index_blob).unwrap();

    // Cluster nodes are registered FIRST so the VG mesh below can carry a
    // REAL appended cluster offset (I1 review point: every prior test dodged
    // representability by hardcoding cluster_table_offset — this one now
    // uses the actual return value of `ClusterBuffer::append`, which is
    // always >= 1 because `ClusterBuffer::new` reserves node 0 as the "no
    // table" sentinel under the C5 XOR rule).
    let mut cluster = ClusterBuffer::new(&ctx1, 8);
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
    let coff_a = cluster.append(ctx1.queue(), &[node1]).expect("first cluster node");
    let coff_b = cluster.append(ctx1.queue(), &[node2]).expect("second cluster node");

    let mut reg = MeshRegistry::new(&ctx1, 8);
    let traditional = traditional_mesh();
    let mut vg = vg_mesh();
    vg.cluster_table_offset = coff_a; // real offset, not the old fictional 100
    let midx_a = reg.register(ctx1.queue(), traditional).expect("traditional mesh");
    let midx_b = reg.register(ctx1.queue(), vg).expect("VG mesh");

    // Snapshot every occupied byte of all four asset buffers before loss.
    let vertex_bytes = off_b as u64 + blob_b.len() as u64;
    let index_bytes = ioff as u64 + index_blob.len() as u64;
    let mesh_bytes_len = 2u64 * 72;
    let cluster_bytes_len = cluster.len() as u64 * 48; // sentinel + 2 appended nodes
    let before_vertex = readback(&ctx1, arena.vertex_buffer(), vertex_bytes);
    let before_index = readback(&ctx1, arena.index_buffer(), index_bytes);
    let before_mesh = readback(&ctx1, reg.buffer(), mesh_bytes_len);
    let before_cluster = readback(&ctx1, cluster.buffer(), cluster_bytes_len);

    // Device loss: drop every GPU-side store, then the entire device. Only
    // the CPU-retained blobs/records (blob_a, blob_b, index_blob, the mesh
    // metadata, the cluster nodes) survive.
    drop(arena);
    drop(reg);
    drop(cluster);
    drop(ctx1);

    // Fresh device; re-drive the real load paths from the retained CPU data.
    let ctx2 = test_context();
    let mut arena2 = GeometryArena::new(&ctx2, 4096, 4096);
    let off_a2 = arena2.upload_vertices(ctx2.queue(), &blob_a).unwrap();
    let off_b2 = arena2.upload_vertices(ctx2.queue(), &blob_b).unwrap();
    let ioff2 = arena2.upload_indices(ctx2.queue(), &index_blob).unwrap();
    assert_eq!((off_a2, off_b2, ioff2), (off_a, off_b, ioff), "fresh arena, same upload order -> same offsets");

    let mut reg2 = MeshRegistry::new(&ctx2, 8);
    let midx_a2 = reg2.register(ctx2.queue(), traditional).expect("traditional mesh re-register");
    let midx_b2 = reg2.register(ctx2.queue(), vg).expect("VG mesh re-register");
    assert_eq!((midx_a2, midx_b2), (midx_a, midx_b), "fresh registry, same register order -> same indices");

    let mut cluster2 = ClusterBuffer::new(&ctx2, 8);
    let coff_a2 = cluster2.append(ctx2.queue(), &[node1]).expect("first cluster node re-append");
    let coff_b2 = cluster2.append(ctx2.queue(), &[node2]).expect("second cluster node re-append");
    assert_eq!((coff_a2, coff_b2), (coff_a, coff_b), "fresh cluster buffer, same append order -> same offsets");

    let after_vertex = readback(&ctx2, arena2.vertex_buffer(), vertex_bytes);
    let after_index = readback(&ctx2, arena2.index_buffer(), index_bytes);
    let after_mesh = readback(&ctx2, reg2.buffer(), mesh_bytes_len);
    let after_cluster = readback(&ctx2, cluster2.buffer(), cluster_bytes_len);

    assert_eq!(after_vertex, before_vertex, "vertex arena byte-identical across device loss");
    assert_eq!(after_index, before_index, "index arena byte-identical across device loss");
    assert_eq!(after_mesh, before_mesh, "mesh SSBO byte-identical across device loss");
    assert_eq!(after_cluster, before_cluster, "cluster SSBO byte-identical across device loss");
}

/// Test 14 (C0 companion): the purpose-built bulk recovery fast path.
/// `MeshRegistry::rebuild` / `ClusterBuffer::rebuild` re-upload the ENTIRE
/// CPU-authoritative copy in one write — the same-device complement to the
/// fresh-device register/append recovery exercised by
/// `test14_assets_device_loss_rematerialization`. Deliberately corrupting the
/// SSBOs first (and readback-confirming the corruption landed) makes the
/// assertion non-vacuous: a `rebuild` that silently no-ops would leave the
/// 0xAB garbage in place and fail loudly.
#[test]
fn rebuild_reuploads_entries_over_corrupted_buffers() {
    let ctx = test_context();

    // 1. Valid data in both stores, readback-verified before corruption.
    let mut reg = MeshRegistry::new(&ctx, 8);
    reg.register(ctx.queue(), traditional_mesh()).expect("traditional mesh");
    reg.register(ctx.queue(), vg_mesh()).expect("VG mesh");
    let mut cluster = ClusterBuffer::new(&ctx, 8);
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
    cluster.append(ctx.queue(), &[node1, node2]).expect("cluster nodes");

    let mesh_len = reg.len() as u64 * 72;
    let cluster_len = cluster.len() as u64 * 48;
    let expected_mesh = mesh_bytes(reg.entries());
    let expected_cluster = cluster_bytes(cluster.nodes());
    assert_eq!(readback(&ctx, reg.buffer(), mesh_len), expected_mesh, "precondition: mesh SSBO valid");
    assert_eq!(readback(&ctx, cluster.buffer(), cluster_len), expected_cluster, "precondition: cluster SSBO valid");

    // 2. Deliberately corrupt both SSBOs over their full occupied extent.
    //    The readbacks force completion (the helper polls to idle) AND prove
    //    the garbage actually landed — no vacuous pass possible.
    ctx.queue().write_buffer(reg.buffer(), 0, &vec![0xAB; mesh_len as usize]);
    ctx.queue().write_buffer(cluster.buffer(), 0, &vec![0xAB; cluster_len as usize]);
    assert_eq!(readback(&ctx, reg.buffer(), mesh_len), vec![0xAB; mesh_len as usize], "corruption landed in mesh SSBO");
    assert_eq!(
        readback(&ctx, cluster.buffer(), cluster_len),
        vec![0xAB; cluster_len as usize],
        "corruption landed in cluster SSBO"
    );

    // 3. The recovery call under test.
    reg.rebuild(ctx.queue());
    cluster.rebuild(ctx.queue());

    // 4. CPU-authoritative state healed VRAM, byte-exact.
    assert_eq!(
        readback(&ctx, reg.buffer(), mesh_len),
        expected_mesh,
        "MeshRegistry::rebuild restored the SSBO from entries()"
    );
    assert_eq!(
        readback(&ctx, cluster.buffer(), cluster_len),
        expected_cluster,
        "ClusterBuffer::rebuild restored the SSBO from nodes()"
    );
}

// ---------------------------------------------------------------------
// TextureStore (M3-α T5, spec §10 G4): SceneDB-owned textures + bindless
// slot table.
// ---------------------------------------------------------------------

/// A small RGBA8 texture descriptor. `COPY_SRC` is required for the
/// readback tests (`copy_texture_to_buffer`); harmless on the others.
fn rgba_desc<'a>(width: u32, height: u32) -> wgpu::TextureDescriptor<'a> {
    wgpu::TextureDescriptor {
        label: Some("texture-store-test"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    }
}

/// Test 13 readback helper: mirrors `readback`'s buffer-to-buffer shape but
/// goes through `copy_texture_to_buffer`, which (unlike `write_texture`)
/// hard-requires `bytes_per_row` to be a multiple of
/// `COPY_BYTES_PER_ROW_ALIGNMENT` (256). Pads the staging buffer's row
/// stride accordingly, then trims the padding back out per-row before
/// returning — so the caller can compare directly against tightly-packed
/// source bytes.
fn readback_texture(
    ctx: &EngineGpuContext,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    bytes_per_pixel: u32,
) -> Vec<u8> {
    let unpadded_bytes_per_row = width * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("texture-readback"),
        size: (padded_bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    ctx.queue().submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let padded = slice.get_mapped_range().expect("mapped range").to_vec();
    staging.unmap();

    // Trim the alignment padding: keep only the first
    // `unpadded_bytes_per_row` bytes of each `padded_bytes_per_row`-wide row.
    let mut out = Vec::with_capacity((unpadded_bytes_per_row * height) as usize);
    for row in 0..height as usize {
        let start = row * padded_bytes_per_row as usize;
        out.extend_from_slice(&padded[start..start + unpadded_bytes_per_row as usize]);
    }
    out
}

#[test]
fn register_two_textures_get_slots_0_and_1() {
    let ctx = test_context();
    let mut store = TextureStore::new(16);
    let slot_a = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[1u8, 2, 3, 4])
        .expect("first register");
    let slot_b = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[5u8, 6, 7, 8])
        .expect("second register");
    assert_eq!((slot_a, slot_b), (0, 1));
    assert_eq!(store.slot_count(), 2);
    assert_eq!(store.upload_count(), 2);
}

#[test]
fn texture_is_present_after_register_and_absent_after_unregister() {
    let ctx = test_context();
    let mut store = TextureStore::new(16);
    let slot = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[9u8, 9, 9, 9])
        .expect("register");
    assert!(store.texture(slot).is_some(), "texture(slot) present right after register");

    store.unregister(slot).expect("unregister");
    assert!(store.texture(slot).is_none(), "texture(slot) absent after unregister — dropped, not just marked");
}

#[test]
fn unregister_then_register_recycles_the_slot_lifo() {
    let ctx = test_context();
    let mut store = TextureStore::new(16);
    let slot_a = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[1u8, 1, 1, 1])
        .expect("first register");
    let slot_b = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[2u8, 2, 2, 2])
        .expect("second register");
    assert_eq!((slot_a, slot_b), (0, 1));

    // Free b then a (LIFO order: a freed last, so a is recycled first).
    store.unregister(slot_b).expect("unregister b");
    store.unregister(slot_a).expect("unregister a");

    let slot_c = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[3u8, 3, 3, 3])
        .expect("recycled register");
    assert_eq!(slot_c, slot_a, "LIFO free list hands back the most-recently-freed slot first");
    assert_eq!(store.slot_count(), 2, "recycling must not grow the slot table extent");
}

#[test]
fn small_store_exhaustion_is_a_hard_error() {
    let ctx = test_context();
    let mut store = TextureStore::new(1);
    assert!(store.register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[0u8; 4]).is_ok());
    let err = store.register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[0u8; 4]);
    assert_eq!(err, Err(TextureError::SlotsExhausted));
}

#[test]
fn unregister_an_already_vacant_slot_is_an_error() {
    let ctx = test_context();
    let mut store = TextureStore::new(16);
    let slot = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[0u8; 4])
        .expect("register");
    store.unregister(slot).expect("first unregister succeeds");
    let err = store.unregister(slot);
    assert_eq!(err, Err(TextureError::SlotVacant), "second unregister of the same slot must fail");
}

#[test]
fn unregister_a_slot_beyond_the_allocated_range_is_an_error() {
    let ctx = test_context();
    let mut store = TextureStore::new(16);
    store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[0u8; 4])
        .expect("register");
    // Only slot 0 has ever been allocated — slot 5 is out of range, not
    // merely vacant.
    let err = store.unregister(5);
    assert_eq!(err, Err(TextureError::SlotOutOfRange));
}

#[test]
fn registered_texture_readback_is_byte_exact_with_row_padding() {
    let ctx = test_context();
    let mut store = TextureStore::new(16);

    // 3px-wide RGBA8: 3 * 4 = 12 bytes/row, which pads to 256 under
    // COPY_BYTES_PER_ROW_ALIGNMENT — exercises the padding/trim path in
    // `readback_texture`, unlike a 64-wide (already-aligned) texture would.
    let width = 3;
    let height = 2;
    let data: Vec<u8> = (0..(width * height * 4) as u8).collect();

    let slot = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(width, height), &data)
        .expect("register");
    let texture = store.texture(slot).expect("texture present");

    let gpu = readback_texture(&ctx, texture, width, height, 4);
    assert_eq!(gpu, data, "readback must be byte-exact vs the uploaded source, padding trimmed");
}

/// Task 5 review follow-up: a free-list-duplication regression test. Kills an
/// implementation that pushes a slot onto the free list BEFORE its
/// `SlotVacant`/`SlotOutOfRange` check fails — such a bug would make
/// `unregister`'s SECOND (erroring) call on an already-vacant slot silently
/// duplicate that slot in the free list, so two subsequent `register` calls
/// would hand out the SAME slot id twice instead of two distinct ones.
#[test]
fn unregister_error_paths_do_not_duplicate_the_free_list() {
    let ctx = test_context();
    let mut store = TextureStore::new(16);

    let slot_a = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[1u8, 1, 1, 1])
        .expect("register a");
    let slot_b = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[2u8, 2, 2, 2])
        .expect("register b");
    assert_eq!((slot_a, slot_b), (0, 1));

    store.unregister(slot_a).expect("first unregister of a succeeds");
    // Second unregister of the same (now-vacant) slot must error AND must
    // not push a second copy of `slot_a` onto the free list.
    let err = store.unregister(slot_a);
    assert_eq!(err, Err(TextureError::SlotVacant));

    store.unregister(slot_b).expect("unregister b succeeds");

    // Free list should now contain exactly {slot_a, slot_b} — one entry
    // each, not two entries for slot_a. Two fresh registers must therefore
    // hand back two DISTINCT slots, not the same slot twice.
    let slot_c = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[3u8, 3, 3, 3])
        .expect("register c (recycled)");
    let slot_d = store
        .register(ctx.device(), ctx.queue(), &rgba_desc(1, 1), &[4u8, 4, 4, 4])
        .expect("register d (recycled)");
    assert_ne!(slot_c, slot_d, "a duplicated free-list entry would hand out the same slot twice");
    assert_eq!(store.slot_count(), 2, "no new slots should have been minted — both recycled");
}

// ---------------------------------------------------------------------
// MeshletBuffer (M3-α T6, C5 amendment / punch-list R12): 32 B meshlet
// records mirroring ClusterBuffer's shape exactly.
// ---------------------------------------------------------------------

/// Byte view of `MeshletEntry` entries for readback comparison. Mirrors the
/// crate-internal `gpu::as_bytes` (pub(crate) — not visible to this
/// integration test binary, which only sees the crate's public API).
///
/// SAFETY: `MeshletEntry` is `#[repr(C)]`, `Copy`, and the crate's own
/// `const _: () = assert!(size_of::<MeshletEntry>() == 32)` pins its layout
/// to exactly 32 bytes with no padding.
fn meshlet_bytes(entries: &[MeshletEntry]) -> Vec<u8> {
    unsafe {
        std::slice::from_raw_parts(entries.as_ptr() as *const u8, std::mem::size_of_val(entries))
    }
    .to_vec()
}

/// Packs (vertex_count, triangle_count) into `counts_packed`'s bits 0..16,
/// leaving the reserved bits 16..32 zero.
fn pack_counts(vertex_count: u8, triangle_count: u8) -> u32 {
    vertex_count as u32 | (triangle_count as u32) << 8
}

fn test_meshlet_entry() -> MeshletEntry {
    MeshletEntry {
        sphere_x: 1.0,
        sphere_y: 2.0,
        sphere_z: 3.0,
        sphere_radius: 0.5,
        cone_packed: 0x2010_08F0,
        data_offset: 128,
        counts_packed: pack_counts(3, 2),
        reserved: 0,
    }
}

#[test]
fn append_two_valid_entries_returns_correct_offsets() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);

    let entry1 = test_meshlet_entry();
    let entry2 = MeshletEntry {
        sphere_x: -1.0,
        sphere_y: -2.0,
        sphere_z: -3.0,
        sphere_radius: 2.0,
        cone_packed: 0x0403_0201,
        data_offset: 256,
        counts_packed: pack_counts(64, 32),
        reserved: 0,
    };

    let offset1 = meshlets.append(ctx.queue(), &[entry1]).expect("first append");
    let offset2 = meshlets.append(ctx.queue(), &[entry2]).expect("second append");

    // Unlike ClusterBuffer, there is no reserved-sentinel node — meshlet
    // offset 0 is a valid real entry (nothing depends on 0 meaning "none").
    assert_eq!(offset1, 0, "first append starts at entry offset 0");
    assert_eq!(offset2, 1, "second append returns offset 1");
    assert_eq!(meshlets.len(), 2);
    assert_eq!(meshlets.get(offset1), &entry1);
    assert_eq!(meshlets.get(offset2), &entry2);
}

#[test]
fn meshlet_entries_readback_byte_exact() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);

    let entry1 = test_meshlet_entry();
    let entry2 = MeshletEntry {
        sphere_x: -1.0,
        sphere_y: -2.0,
        sphere_z: -3.0,
        sphere_radius: 2.0,
        cone_packed: 0x0403_0201,
        data_offset: 256,
        counts_packed: pack_counts(64, 32),
        reserved: 0,
    };

    meshlets.append(ctx.queue(), &[entry1]).expect("first append");
    meshlets.append(ctx.queue(), &[entry2]).expect("second append");

    let gpu = readback(&ctx, meshlets.buffer(), 2 * 32);
    let expected = meshlet_bytes(meshlets.entries());
    assert_eq!(expected.len(), 64, "two 32-byte records");
    assert_eq!(gpu, expected, "SSBO bytes must exactly mirror as_bytes(entries())");
}

#[test]
fn append_rejects_zero_radius() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    bad.sphere_radius = 0.0;
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::InvalidRadius));
    assert_eq!(meshlets.len(), 0, "rejected entry must not consume an offset");
}

#[test]
fn append_rejects_negative_radius() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    bad.sphere_radius = -3.0;
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::InvalidRadius));
    assert_eq!(meshlets.len(), 0);
}

#[test]
fn append_rejects_negative_zero_radius() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    // -0.0 == 0.0 under IEEE-754, and `-0.0 > 0.0` is false, so the
    // NaN-rejecting `!(r > 0.0)` form correctly rejects it too (radius must
    // be strictly positive, not merely non-negative).
    bad.sphere_radius = -0.0f32;
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::InvalidRadius));
    assert_eq!(meshlets.len(), 0);
}

#[test]
fn append_rejects_nan_radius() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    bad.sphere_radius = f32::NAN;
    // IEEE-754: `NaN > 0.0` is false, so `!(r > 0.0)` is true -> rejected.
    // A naive `r <= 0.0` check would have missed this (`NaN <= 0.0` is also
    // false), silently ACCEPTING the NaN radius.
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::InvalidRadius));
    assert_eq!(meshlets.len(), 0, "NaN radius must not consume an offset");
}

#[test]
fn append_accepts_positive_infinity_radius() {
    // Documents the boundary of the `!(r > 0.0)` rule: it rejects
    // non-positive and NaN radii, but `+inf > 0.0` is true, so a +inf radius
    // is (deliberately) accepted — the validation rule is "strictly
    // positive", not "finite".
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut entry = test_meshlet_entry();
    entry.sphere_radius = f32::INFINITY;
    let offset = meshlets.append(ctx.queue(), &[entry]).expect("+inf radius is > 0.0, accepted");
    assert_eq!(offset, 0);
    assert_eq!(meshlets.len(), 1);
}

#[test]
fn append_rejects_reserved_nonzero() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    bad.reserved = 1;
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::ReservedNonZero));
    assert_eq!(meshlets.len(), 0);
}

#[test]
fn append_rejects_counts_packed_reserved_bits_nonzero() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    bad.counts_packed = pack_counts(3, 2) | (1u32 << 16); // reserved bit 16 set
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::CountsReservedNonZero));
    assert_eq!(meshlets.len(), 0);
}

#[test]
fn append_rejects_zero_vertex_count() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    bad.counts_packed = pack_counts(0, 2);
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::EmptyCounts));
    assert_eq!(meshlets.len(), 0);
}

#[test]
fn append_rejects_zero_triangle_count() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 4);
    let mut bad = test_meshlet_entry();
    bad.counts_packed = pack_counts(3, 0);
    let err = meshlets.append(ctx.queue(), &[bad]);
    assert_eq!(err, Err(MeshletError::EmptyCounts));
    assert_eq!(meshlets.len(), 0);
}

#[test]
fn meshlet_batched_appends_return_sequential_offsets_and_read_back_byte_exact() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 8);

    let entry1 = test_meshlet_entry();
    let entry2 = MeshletEntry {
        sphere_x: -1.0,
        sphere_y: -2.0,
        sphere_z: -3.0,
        sphere_radius: 2.0,
        cone_packed: 0x0403_0201,
        data_offset: 256,
        counts_packed: pack_counts(64, 32),
        reserved: 0,
    };
    let entry3 = MeshletEntry {
        sphere_x: 5.0,
        sphere_y: 6.0,
        sphere_z: 7.0,
        sphere_radius: 1.5,
        cone_packed: 0x0807_0605,
        data_offset: 512,
        counts_packed: pack_counts(1, 1),
        reserved: 0,
    };

    let offset_a = meshlets.append(ctx.queue(), &[entry1, entry2]).expect("2-entry batch");
    let offset_b = meshlets.append(ctx.queue(), &[entry3]).expect("1-entry batch");
    assert_eq!(offset_a, 0, "first batch starts at entry offset 0");
    assert_eq!(offset_b, 2, "second batch starts at entry offset 2");
    assert_eq!(meshlets.len(), 3);
    assert_eq!(meshlets.entries(), [entry1, entry2, entry3]);

    let gpu = readback(&ctx, meshlets.buffer(), 3 * 32);
    let expected = meshlet_bytes(meshlets.entries());
    assert_eq!(expected.len(), 96, "three 32-byte records");
    assert_eq!(gpu, expected, "SSBO bytes must exactly mirror as_bytes(entries())");
}

#[test]
fn meshlet_append_fails_when_buffer_full() {
    let ctx = test_context();
    let mut meshlets = MeshletBuffer::new(&ctx, 2);
    let entry = test_meshlet_entry();

    assert!(meshlets.append(ctx.queue(), &[entry]).is_ok());
    assert!(meshlets.append(ctx.queue(), &[entry]).is_ok());
    let err = meshlets.append(ctx.queue(), &[entry]);
    assert_eq!(err, Err(MeshletError::BufferFull));
    assert_eq!(meshlets.len(), 2, "full buffer rejects without growing");
}

/// Test 14 (C0 companion): corruption-heal rebuild — mirrors
/// `rebuild_reuploads_entries_over_corrupted_buffers`'s shape for
/// `MeshletBuffer`. Deliberately corrupting the SSBO first (and
/// readback-confirming the corruption landed) makes the assertion
/// non-vacuous.
#[test]
fn meshlet_rebuild_reuploads_entries_over_corrupted_buffer() {
    let ctx = test_context();

    let mut meshlets = MeshletBuffer::new(&ctx, 8);
    let entry1 = test_meshlet_entry();
    let entry2 = MeshletEntry {
        sphere_x: -1.0,
        sphere_y: -2.0,
        sphere_z: -3.0,
        sphere_radius: 2.0,
        cone_packed: 0x0403_0201,
        data_offset: 256,
        counts_packed: pack_counts(64, 32),
        reserved: 0,
    };
    meshlets.append(ctx.queue(), &[entry1, entry2]).expect("entries");

    let len = meshlets.len() as u64 * 32;
    let expected = meshlet_bytes(meshlets.entries());
    assert_eq!(readback(&ctx, meshlets.buffer(), len), expected, "precondition: meshlet SSBO valid");

    // Deliberately corrupt the SSBO over its full occupied extent.
    ctx.queue().write_buffer(meshlets.buffer(), 0, &vec![0xCD; len as usize]);
    assert_eq!(readback(&ctx, meshlets.buffer(), len), vec![0xCD; len as usize], "corruption landed in meshlet SSBO");

    // The recovery call under test.
    meshlets.rebuild(ctx.queue());

    assert_eq!(
        readback(&ctx, meshlets.buffer(), len),
        expected,
        "MeshletBuffer::rebuild restored the SSBO from entries()"
    );
}
