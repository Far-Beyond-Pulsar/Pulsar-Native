//! Asset behavior of `StaticMeshComponent` (Pulsar-Native#1035 acceptance,
//! #1081, SceneDB#84): missing and corrupt assets, rapid replacement while
//! loads are in flight, shared asset reuse, a property-only edit that
//! neither reloads the asset nor re-uploads the geometry, and reload when
//! the file changes.
//!
//! A component names its mesh by `AssetId`; the world loads it on a worker
//! thread and binds the geometry (`World::pump_assets`). Nothing is read
//! from disk inside a write.
//!
//! One test, because the project path the loader resolves against is a
//! process global. Needs a GPU adapter for the upload checks (lavapipe
//! works); skips them without one.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use helio_component::components::ObjectMovability;
use meshes_component::components::{
    MESH_ASSET_KIND, StaticMeshComponent, mesh_asset_residency, mesh_source_for_file,
    prefetch_mesh_asset,
};
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};
use pulsar_scenedb::{AssetState, World};

fn primitive(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../assets/meshes/primitives")
        .join(name)
}

fn context() -> Option<EngineGpuContext> {
    // Creating devices from many tests at once stalls some drivers; take turns.
    static DEVICE_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _device_creation = DEVICE_CREATION.lock().unwrap_or_else(|p| p.into_inner());
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    Some(EngineGpuContext::new(Arc::new(device), Arc::new(queue)))
}

fn set_mesh_asset(world: &mut World, entity: pulsar_scenedb::Entity, path: &str) {
    pulsar_world_registry::set_world_component_property(
        "StaticMeshComponent",
        world,
        entity,
        "mesh_asset",
        Box::new(meshes_component::components::MeshAssetPath::new(path)),
    )
    .unwrap_or_else(|_| panic!("mesh_asset write to {path} refused"));
}

fn bytes(vertices: &[helio_mesh_api::PackedVertex]) -> &[u8] {
    bytemuck::cast_slice(vertices)
}

fn vertex_count(world: &World, entity: pulsar_scenedb::Entity) -> usize {
    world
        .get::<StaticMeshComponent>(entity)
        .unwrap()
        .vertices
        .len()
}

fn resolve(world: &mut World) {
    assert!(
        world.resolve_assets_blocking(Duration::from_secs(30)),
        "mesh assets did not load"
    );
}

/// The geometry the mesh at `path` loads to, through a world of its own.
fn loaded(path: &str) -> StaticMeshComponent {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, StaticMeshComponent::for_mesh_asset(path));
    resolve(&mut world);
    world.get::<StaticMeshComponent>(entity).unwrap().clone()
}

#[test]
fn mesh_assets_load_replace_and_survive_bad_input() {
    let project = tempfile::tempdir().unwrap();
    let meshes = project.path().join("meshes");
    std::fs::create_dir_all(&meshes).unwrap();
    std::fs::copy(primitive("SM_Cube.fbx"), meshes.join("cube.fbx")).unwrap();
    std::fs::copy(primitive("SM_Sphere.fbx"), meshes.join("sphere.fbx")).unwrap();
    std::fs::copy(primitive("SM_Torus.fbx"), meshes.join("torus.fbx")).unwrap();
    std::fs::write(meshes.join("corrupt.fbx"), b"not a mesh at all").unwrap();
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());

    // Naming a mesh reads nothing: the value carries the asset id only.
    let unbound = StaticMeshComponent::for_mesh_asset("meshes/cube.fbx");
    assert!(!unbound.mesh.is_zero());
    assert!(unbound.vertices.is_empty());
    assert_eq!(
        unbound.mesh.source().map(|(kind, _)| kind),
        Some(MESH_ASSET_KIND)
    );

    let cube = loaded("meshes/cube.fbx");
    let sphere = loaded("meshes/sphere.fbx");
    let torus = loaded("meshes/torus.fbx");
    assert!(!cube.vertices.is_empty() && !sphere.vertices.is_empty());
    assert_ne!(
        cube.vertices.len(),
        sphere.vertices.len(),
        "distinct fixtures"
    );
    assert_ne!(cube.bounds_local, [0.0, 0.0, 0.0, 0.5]);

    // LOD demand and ordinary strong AssetId references share SceneDB's
    // byte-budgeted residency cache instead of parsing duplicate CPU meshes.
    let cube_residency = mesh_asset_residency(cube.mesh).expect("declared mesh residency");
    let cube_payload = cube_residency.get().expect("resident cube mesh");
    assert_eq!(
        cube_payload.upload.geometry.vertices.len(),
        cube.vertices.len()
    );
    assert!(Arc::ptr_eq(
        &cube_payload,
        &cube_residency.get().expect("cached cube mesh")
    ));
    let sphere_prefetch = prefetch_mesh_asset(sphere.mesh).expect("sphere prefetch");
    let sphere_payload = sphere_prefetch.wait().expect("prefetched sphere mesh");
    assert_eq!(
        sphere_payload.upload.geometry.vertices.len(),
        sphere.vertices.len()
    );

    // ── Missing and corrupt assets: an empty, invisible mesh, no panic ────
    for path in ["meshes/missing.fbx", "meshes/corrupt.fbx"] {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, StaticMeshComponent::for_mesh_asset(path));
        resolve(&mut world);
        let mesh = world.get::<StaticMeshComponent>(entity).unwrap();
        assert!(
            matches!(world.asset_state(mesh.mesh), AssetState::Failed(_)),
            "{path}"
        );
        assert!(
            mesh.vertices.is_empty() && mesh.indices.is_empty(),
            "{path}"
        );
        assert_eq!(
            mesh.bounds_local,
            [0.0, 0.0, 0.0, 0.5],
            "{path}: the no-mesh bounds"
        );
        // The boundary decoder accepts it too: the path is kept, for a later fix.
        let decoded = pulsar_world_registry::decode_world_component_value(
            "StaticMeshComponent",
            &serde_json::json!({ "mesh_asset": path }),
        )
        .expect("registered")
        .expect("a bad asset is not a decode error");
        let decoded = decoded.downcast::<StaticMeshComponent>().unwrap();
        assert_eq!(decoded.mesh_asset.as_str(), path);
        assert_eq!(decoded.mesh, mesh.mesh);
        assert!(decoded.vertices.is_empty());
    }

    let mut world = World::new();
    let ctx = context();
    let store = ctx.as_ref().map(|ctx| {
        let store = Arc::new(SceneGpuStore::new(
            ctx,
            SceneGpuConfig {
                classes: vec![],
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        ));
        world.attach_gpu_mirror(GpuMirrorHandle::new(
            Arc::clone(&store),
            Arc::clone(ctx.queue()),
        ));
        store
    });
    let flush = |world: &World| {
        ctx.as_ref()
            .map(|ctx| world.flush_gpu_mirror(ctx.queue()).unwrap())
    };

    // ── Rapid replacement: the last write wins, whatever is in flight ─────
    let entity = world.spawn();
    world.insert(
        entity,
        StaticMeshComponent::for_mesh_asset("meshes/cube.fbx"),
    );
    world.pump_assets();
    for path in [
        "meshes/sphere.fbx",
        "meshes/missing.fbx",
        "meshes/torus.fbx",
        "meshes/sphere.fbx",
    ] {
        set_mesh_asset(&mut world, entity, path);
        world.pump_assets();
    }
    resolve(&mut world);
    assert_eq!(vertex_count(&world, entity), sphere.vertices.len());
    assert_eq!(
        bytes(&world.get::<StaticMeshComponent>(entity).unwrap().vertices),
        bytes(&sphere.vertices)
    );
    flush(&world);

    // ── Shared reuse: two components of one asset share one GPU allocation ─
    let other = world.spawn();
    world.insert(
        other,
        StaticMeshComponent::for_mesh_asset("meshes/sphere.fbx"),
    );
    resolve(&mut world);
    flush(&world);
    if let Some(store) = &store {
        let handle =
            |e: pulsar_scenedb::Entity| StaticMeshComponent::vertices_gpu_handle(store, e.index());
        assert_eq!(
            handle(entity),
            handle(other),
            "one asset, one geometry allocation"
        );
    }

    // ── A property-only edit: no reload, no geometry upload ───────────────
    std::fs::remove_file(meshes.join("sphere.fbx")).unwrap();
    pulsar_world_registry::set_world_component_property(
        "StaticMeshComponent",
        &mut world,
        entity,
        "movability",
        Box::new(ObjectMovability::Movable),
    )
    .unwrap();
    assert!(!world.asset_work_pending(), "the edit asked for no load");
    assert_eq!(
        vertex_count(&world, entity),
        sphere.vertices.len(),
        "the edit did not reload the (now deleted) asset"
    );
    if let Some(stats) = flush(&world) {
        let geometry_bytes = std::mem::size_of_val(sphere.vertices.as_slice()) as u64;
        assert!(
            stats.bytes < geometry_bytes / 10,
            "a movability edit uploaded {} bytes; the geometry is {geometry_bytes}",
            stats.bytes
        );
    }

    // ── Reload: the file changes, every component showing it follows ──────
    std::fs::copy(primitive("SM_Torus.fbx"), meshes.join("sphere.fbx")).unwrap();
    let source = mesh_source_for_file(&meshes.join("sphere.fbx"));
    assert_eq!(world.asset_source_changed(&source), 1);
    resolve(&mut world);
    for e in [entity, other] {
        assert_eq!(
            bytes(&world.get::<StaticMeshComponent>(e).unwrap().vertices),
            bytes(&torus.vertices),
            "a changed file is loaded again and rebound"
        );
    }
    flush(&world);
    if let Some(store) = &store {
        let handle =
            |e: pulsar_scenedb::Entity| StaticMeshComponent::vertices_gpu_handle(store, e.index());
        assert_eq!(handle(entity), handle(other), "still one allocation");
    }
}
