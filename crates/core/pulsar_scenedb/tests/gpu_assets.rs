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
