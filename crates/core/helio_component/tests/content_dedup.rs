//! End-to-end content-dedup adversarial coverage (Pulsar-Native#632/#661) on
//! the REAL `StaticMeshComponent`/`MeshAssetPath` production types — not a
//! synthetic fixture (SceneDB's own `interned_var_len.rs` already proves the
//! generic mechanism against a synthetic `ContentAddressed` type; this file
//! proves `MeshAssetPath`'s OWN content-id resolution chain end to end).
//!
//! `MeshAssetPath::content_id` resolves through `engine_state`'s process-
//! global project path, so every test in this file shares ONE project root
//! (set exactly once, via `INIT`, and never mutated again) rather than each
//! setting its own -- `cargo test` runs tests as concurrent threads in one
//! process by default, and a per-test project-path mutation would race.
//! Each test instead writes its own uniquely-named `.mesh` file under the
//! shared root, so tests stay independent without needing `--test-threads=1`.

#![cfg(feature = "gpu")]

use helio::PackedVertex;
use helio_component::components::{MeshAssetPath, MeshGeometryKey, StaticMeshComponent};
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};
use pulsar_scenedb::{Entity, World};
use std::sync::{Arc, Once};

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
        label: Some("content-dedup-test"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
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

static INIT: Once = Once::new();

/// The shared project root every test in this file resolves `mesh_asset`
/// paths against — created once, never removed (process-lifetime temp dir;
/// `std::env::temp_dir()` is cleaned by the OS eventually, not by us, same
/// as any other test fixture that doesn't bother with `Drop`-based cleanup).
fn project_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join("pulsar_content_dedup_test_root");
    INIT.call_once(|| {
        std::fs::create_dir_all(&root).expect("create test project root");
        // `set_project_path` is a no-op without a global `EngineContext`
        // already installed (`EngineContext::global()` returns `None` and
        // it silently does nothing) — a bare test binary never bootstraps
        // one the way the real editor app does, so this test harness has
        // to install a minimal one itself.
        engine_state::EngineContext::new().set_global();
        engine_state::set_project_path(root.to_string_lossy().into_owned());
    });
    root
}

/// Writes a fresh native `.mesh` v2 asset under the shared project root with
/// a unique name (so concurrently-running tests never collide), containing
/// `vert_count` vertices. Returns the path relative to the project root, the
/// shape `mesh_asset` fields actually store.
fn write_mesh_asset(name: &str, vert_count: usize) -> String {
    let root = project_root();
    let mesh = helio::MeshUpload {
        vertices: (0..vert_count)
            .map(|i| PackedVertex {
                position: [i as f32, 0.0, 0.0],
                ..Default::default()
            })
            .collect(),
        indices: (0..vert_count as u32).collect(),
    };
    let id = helio_component::mesh_cache::content_id_for_bytes(&mesh);
    let bytes = helio_component::mesh_cache::encode(&mesh, id);
    let rel = format!("{name}.mesh");
    std::fs::write(root.join(&rel), bytes).expect("write test mesh asset");
    rel
}

fn mirrored_world() -> (World, Arc<SceneGpuStore>, EngineGpuContext) {
    let ctx = test_context();
    let mut store = SceneGpuStore::new(&ctx, scene_cfg());
    StaticMeshComponent::register_gpu_columns_growable(&mut store, 16, ctx.device());
    let store = Arc::new(store);
    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));
    (world, store, ctx)
}

fn vertices_pool(
    store: &SceneGpuStore,
) -> Arc<pulsar_scenedb::gpu::InternedVarLenPool<PackedVertex>> {
    store
        .interned_var_len_pool::<PackedVertex>(pulsar_scenedb::gpu::BufferKey::of(
            "builtin_mesh_vertex",
        ))
        .expect("registered on first insert")
}

fn spawn_with_asset(world: &mut World, asset: &str, _vert_count: usize) -> Entity {
    let e = world.spawn();
    let upload = helio_component::subsystems::load_mesh_upload(&project_root().join(asset))
        .expect("load fixture mesh");
    world.insert(
        e,
        StaticMeshComponent {
            mesh_asset: MeshAssetPath::new(asset),
            geometry_key: MeshGeometryKey::for_path(&MeshAssetPath::new(asset)),
            vertices: upload.vertices,
            indices: upload.indices,
            bounds_local: [0.0, 0.0, 0.0, 0.5],
            ..Default::default()
        },
    );
    e
}

#[test]
fn ten_entities_referencing_the_same_asset_share_one_gpu_allocation() {
    let (mut world, store, _ctx) = mirrored_world();
    let asset = write_mesh_asset("shared_cube", 4);

    let entities: Vec<Entity> = (0..10)
        .map(|_| spawn_with_asset(&mut world, &asset, 4))
        .collect();

    let expected_id = pulsar_scenedb::handle_ledger::HandleId(
        helio_component::mesh_cache::content_id_for_path(&project_root().join(&asset)).unwrap(),
    );
    let vpool = vertices_pool(&store);
    assert_eq!(
        vpool.audit(),
        vec![(
            expected_id,
            10,
            4 * std::mem::size_of::<PackedVertex>() as u64
        )]
    );

    let handles: Vec<_> = entities
        .iter()
        .map(|e| StaticMeshComponent::vertices_gpu_handle(&store, e.index()).unwrap())
        .collect();
    assert!(
        handles.iter().all(|h| *h == handles[0]),
        "all ten entities resolve to the identical shared range"
    );
}

#[test]
fn leak_loop_50_rounds_of_100_entities_across_4_assets_leaves_nothing_resident() {
    // Mandatory (Pulsar-Native#632/#661): spawn a batch referencing a small
    // set of real assets, despawn it all, repeat — audit must be empty
    // every round, not just eventually.
    let (mut world, store, _ctx) = mirrored_world();
    let assets: Vec<String> = (0..4)
        .map(|k| write_mesh_asset(&format!("leak_loop_asset_{k}"), 3 + k))
        .collect();

    for round in 0..50 {
        let entities: Vec<Entity> = (0..100)
            .map(|i| {
                spawn_with_asset(
                    &mut world,
                    &assets[i % assets.len()],
                    3 + (i % assets.len()),
                )
            })
            .collect();

        let vpool = vertices_pool(&store);
        assert_eq!(
            vpool.audit().len(),
            assets.len(),
            "round {round}: exactly the 4 fixture assets resident"
        );

        for e in entities {
            world.despawn(e);
        }
        assert!(
            vertices_pool(&store).audit().is_empty(),
            "round {round}: leaked vertex residency"
        );
    }
}

#[test]
fn removing_the_component_without_despawning_frees_correctly() {
    let (mut world, store, _ctx) = mirrored_world();
    let asset = write_mesh_asset("remove_not_despawn", 5);
    let a = spawn_with_asset(&mut world, &asset, 5);
    let b = spawn_with_asset(&mut world, &asset, 5);

    let vpool = vertices_pool(&store);
    assert_eq!(vpool.audit()[0].1, 2, "two references");

    world.remove::<StaticMeshComponent>(a);
    assert!(world.is_alive(a), "remove must not despawn the entity");
    assert_eq!(vertices_pool(&store).audit()[0].1, 1);

    world.despawn(b);
    assert!(vertices_pool(&store).audit().is_empty());
}

#[test]
fn file_mutation_between_hydrates_mints_a_new_id_and_the_old_refcount_falls() {
    let root = project_root();
    let rel = "mutating_asset.mesh".to_string();
    let mesh_v1 = helio::MeshUpload {
        vertices: vec![
            PackedVertex {
                position: [1.0, 0.0, 0.0],
                ..Default::default()
            };
            3
        ],
        indices: vec![0, 1, 2],
    };
    let id_v1 = helio_component::mesh_cache::content_id_for_bytes(&mesh_v1);
    std::fs::write(
        root.join(&rel),
        helio_component::mesh_cache::encode(&mesh_v1, id_v1),
    )
    .unwrap();

    let (mut world, store, _ctx) = mirrored_world();
    let e1 = spawn_with_asset(&mut world, &rel, 3);
    let vpool = vertices_pool(&store);
    assert_eq!(vpool.audit().len(), 1);
    let first_id = vpool.audit()[0].0;

    // Mutate the file: different geometry, re-encoded fresh (a real content
    // id, not a stale/backfilled one) — must be a DIFFERENT id.
    std::thread::sleep(std::time::Duration::from_millis(10)); // ensure mtime actually advances on coarse filesystems
    let mesh_v2 = helio::MeshUpload {
        vertices: vec![
            PackedVertex {
                position: [9.0, 0.0, 0.0],
                ..Default::default()
            };
            5
        ],
        indices: vec![0, 1, 2, 3, 4],
    };
    let id_v2 = helio_component::mesh_cache::content_id_for_bytes(&mesh_v2);
    assert_ne!(id_v1, id_v2);
    std::fs::write(
        root.join(&rel),
        helio_component::mesh_cache::encode(&mesh_v2, id_v2),
    )
    .unwrap();

    // A second entity referencing the SAME path now resolves the NEW id.
    let e2 = spawn_with_asset(&mut world, &rel, 5);
    let audit = vpool.audit();
    assert_eq!(
        audit.len(),
        2,
        "old and new ids coexist while both are referenced"
    );
    assert!(
        audit
            .iter()
            .any(|(id, refs, _)| *id == first_id && *refs == 1),
        "e1 still holds the OLD id"
    );

    let h1 = StaticMeshComponent::vertices_gpu_handle(&store, e1.index()).unwrap();
    let h2 = StaticMeshComponent::vertices_gpu_handle(&store, e2.index()).unwrap();
    assert_ne!(
        h1, h2,
        "e1 and e2 must resolve to DIFFERENT ranges after the file changed"
    );
}

#[test]
fn determinism_the_same_scene_loaded_twice_produces_identical_residency() {
    let asset = write_mesh_asset("determinism_asset", 6);

    let (mut world_a, store_a, _ctx_a) = mirrored_world();
    let entities_a: Vec<Entity> = (0..7)
        .map(|_| spawn_with_asset(&mut world_a, &asset, 6))
        .collect();
    let audit_a = vertices_pool(&store_a).audit();

    let (mut world_b, store_b, _ctx_b) = mirrored_world();
    let entities_b: Vec<Entity> = (0..7)
        .map(|_| spawn_with_asset(&mut world_b, &asset, 6))
        .collect();
    let audit_b = vertices_pool(&store_b).audit();

    assert_eq!(
        audit_a, audit_b,
        "loading the identical scene twice must produce identical residency (id, refcount, bytes)"
    );
    assert_eq!(entities_a.len(), entities_b.len());
}
