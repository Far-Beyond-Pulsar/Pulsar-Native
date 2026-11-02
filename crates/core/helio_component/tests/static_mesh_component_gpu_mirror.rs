//! Proves `StaticMeshComponent`'s `#[gpu] Vec<PackedVertex>`/`Vec<u32>`
//! fields (Pulsar-Native#561 Phase D) actually GPU-mirror through `World`,
//! on the real production component -- not a synthetic test fixture like
//! `pulsar_scenedb`'s own `world_gpu_mirror_var_len.rs`/`engine_class_derive`'s
//! `scene_store_delegation.rs`, both of which prove the *mechanism* in
//! isolation. This is the first component to combine a var-len `#[gpu]`
//! field with an ordinary, un-mirrored field (`mesh_asset`) on the same
//! struct -- a real, if expected, new combination worth a real test rather
//! than an inference from the mechanism tests alone.
//!
//! Does NOT exercise `hydrate_static_mesh_component` itself (that needs a
//! real project path + mesh file on disk, i.e. an asset-loading test, not a
//! GPU-mirror one) -- these tests populate `vertices`/`indices` directly and
//! insert via `World::insert`, the same real entry point hydrate itself
//! calls after loading.

use helio::PackedVertex;
use helio_component::components::{MeshAssetPath, MeshGeometryKey, StaticMeshComponent};
use pulsar_scenedb::World;
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};
use std::sync::Arc;

fn test_context() -> EngineGpuContext {
    // Creating devices from many tests at once stalls some drivers; take turns.
    static DEVICE_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _device_creation = DEVICE_CREATION.lock().unwrap_or_else(|p| p.into_inner());
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU tests need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("static-mesh-component-gpu-mirror-test"),
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
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("mapped range").to_vec();
    staging.unmap();
    data
}

fn scene_cfg() -> SceneGpuConfig {
    SceneGpuConfig {
        classes: vec![RegionClassConfig {
            capacity: 64,
            max_resident_cells: 1,
        }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    }
}

fn v(x: f32) -> PackedVertex {
    PackedVertex {
        position: [x, 0.0, 0.0],
        ..Default::default()
    }
}

#[test]
fn vertices_and_indices_land_in_their_shared_gpu_pools_through_world_insert() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    StaticMeshComponent::register_gpu_columns_growable(&mut store, 16, ctx.device());
    let store = Arc::new(store);

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));

    let entity = world.spawn();
    world.insert(
        entity,
        StaticMeshComponent {
            mesh_asset: MeshAssetPath::new("meshes/primitives/SM_Cube.fbx"),
            geometry_key: MeshGeometryKey::for_path(&MeshAssetPath::new(
                "meshes/primitives/SM_Cube.fbx",
            )),
            vertices: vec![v(1.0), v(2.0), v(3.0)],
            indices: vec![0, 1, 2],
            bounds_local: [0.0, 0.0, 0.0, 0.5],
            ..Default::default()
        },
    );
    world
        .flush_gpu_mirror(ctx.queue())
        .expect("mirror attached");

    // Real GPU-side readback, not just the CPU World row, from the pools the
    // fields declare (`#[gpu(buffer = "builtin_mesh_vertex")]` /
    // `"builtin_mesh_index"`), the same keys the render passes read.
    let vertex_pool = store
        .interned_var_len_pool::<PackedVertex>(pulsar_scenedb::gpu::BufferKey::of(
            "builtin_mesh_vertex",
        ))
        .expect("vertices pool registered on first insert")
        .underlying()
        .clone();
    let index_pool = store
        .interned_var_len_pool::<u32>(pulsar_scenedb::gpu::BufferKey::of("builtin_mesh_index"))
        .expect("indices pool registered on first insert")
        .underlying()
        .clone();

    let mut vertex_bytes = Vec::new();
    vertex_pool.with_buffer(&mut |b| {
        vertex_bytes = readback(&ctx, b, 3 * std::mem::size_of::<PackedVertex>() as u64)
    });
    let got_x: Vec<f32> = vertex_bytes
        .chunks(std::mem::size_of::<PackedVertex>())
        .map(|c| f32::from_ne_bytes(c[0..4].try_into().unwrap()))
        .collect();
    assert_eq!(
        got_x,
        vec![1.0, 2.0, 3.0],
        "vertex data must actually be GPU-resident, not just held CPU-side"
    );

    let mut index_bytes = Vec::new();
    index_pool.with_buffer(&mut |b| index_bytes = readback(&ctx, b, 3 * 4));
    let got_indices: Vec<u32> = index_bytes
        .chunks(4)
        .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
        .collect();
    assert_eq!(got_indices, vec![0, 1, 2]);

    let stored = world
        .get::<StaticMeshComponent>(entity)
        .expect("component must be readable back");
    assert_eq!(
        stored.mesh_asset.as_str(),
        "meshes/primitives/SM_Cube.fbx",
        "the plain, non-#[gpu] field must round-trip normally alongside the var-len ones"
    );
}

#[test]
fn re_insert_with_different_length_data_still_round_trips_through_world() {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    StaticMeshComponent::register_gpu_columns_growable(&mut store, 4, ctx.device());
    let store = Arc::new(store);

    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));

    let entity = world.spawn();
    world.insert(
        entity,
        StaticMeshComponent {
            mesh_asset: MeshAssetPath::new("meshes/primitives/SM_Cube.fbx"),
            geometry_key: MeshGeometryKey::for_path(&MeshAssetPath::new(
                "meshes/primitives/SM_Cube.fbx",
            )),
            vertices: vec![v(1.0), v(2.0), v(3.0), v(4.0)],
            indices: (0..6).collect(),
            bounds_local: [0.0, 0.0, 0.0, 0.5],
            ..Default::default()
        },
    );
    world
        .flush_gpu_mirror(ctx.queue())
        .expect("mirror attached");

    // Swap to a different mesh asset with a DIFFERENT vertex/index count --
    // proves a re-insert (e.g. the user re-picking a different mesh in the
    // properties panel) doesn't panic or corrupt the pool, same guarantee
    // `world_gpu_mirror_var_len.rs` proves for the synthetic test type.
    world.insert(
        entity,
        StaticMeshComponent {
            mesh_asset: MeshAssetPath::new("meshes/primitives/SM_Sphere.fbx"),
            geometry_key: MeshGeometryKey::for_path(&MeshAssetPath::new(
                "meshes/primitives/SM_Sphere.fbx",
            )),
            vertices: vec![v(9.0)],
            indices: vec![0],
            bounds_local: [0.0, 0.0, 0.0, 0.5],
            ..Default::default()
        },
    );
    world
        .flush_gpu_mirror(ctx.queue())
        .expect("mirror attached");

    let stored = world.get::<StaticMeshComponent>(entity).unwrap();
    assert_eq!(stored.vertices.len(), 1);
    assert_eq!(
        stored.mesh_asset.as_str(),
        "meshes/primitives/SM_Sphere.fbx"
    );
}
