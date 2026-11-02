//! A world rebuilds its GPU copies from the asset data it holds (SceneDB#84):
//! after a device is lost, attaching a mirror on a new device replays every
//! component, and the meshes and images the world bound are uploaded again
//! from their CPU copies, with the source files gone.
//!
//! Needs a GPU adapter (lavapipe works); skips without one.

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use helio_component::components::{SpriteComponent, StaticMeshComponent};
use pulsar_scenedb::World;
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore, TextureStore,
};

fn context() -> Option<EngineGpuContext> {
    // Creating devices from many tests at once stalls some drivers; take turns.
    static DEVICE_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _device_creation = DEVICE_CREATION.lock().unwrap_or_else(|p| p.into_inner());
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    Some(EngineGpuContext::new(Arc::new(device), Arc::new(queue)))
}

/// A mirror on `ctx`'s device with a texture store of its own.
fn mirror(
    ctx: &EngineGpuContext,
) -> (
    GpuMirrorHandle,
    Arc<SceneGpuStore>,
    Arc<RwLock<TextureStore>>,
) {
    let store = Arc::new(SceneGpuStore::new(
        ctx,
        SceneGpuConfig {
            classes: vec![],
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    ));
    let textures = Arc::new(RwLock::new(TextureStore::new(64)));
    let handle = GpuMirrorHandle::new(Arc::clone(&store), Arc::clone(ctx.queue()))
        .with_texture_store(Arc::clone(&textures))
        .expect("a texture store on the mirror's device");
    (handle, store, textures)
}

/// The sizes of the textures `textures` holds.
fn resident(textures: &RwLock<TextureStore>) -> Vec<(u32, u32)> {
    let textures = textures.read().unwrap();
    (0..64)
        .filter_map(|slot| textures.texture(slot))
        .map(|texture| (texture.width(), texture.height()))
        .collect()
}

#[test]
fn a_new_device_gets_every_bound_asset_again_without_the_files() {
    let Some(first) = context() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("images")).unwrap();
    std::fs::create_dir_all(project.path().join("meshes")).unwrap();
    image::save_buffer(
        project.path().join("images/sheet.png"),
        &[255u8; 3 * 2 * 4],
        3,
        2,
        image::ColorType::Rgba8,
    )
    .unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../assets/meshes/primitives/SM_Cube.fbx"),
        project.path().join("meshes/cube.fbx"),
    )
    .unwrap();
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());

    let mut world = World::new();
    let (handle, _store, first_textures) = mirror(&first);
    world.attach_gpu_mirror(handle);
    let sprite = world.spawn();
    world.insert(
        sprite,
        SpriteComponent {
            texture: "images/sheet.png".into(),
            ..Default::default()
        },
    );
    let mesh = world.spawn();
    world.insert(mesh, StaticMeshComponent::for_mesh_asset("meshes/cube.fbx"));
    assert!(world.resolve_assets_blocking(Duration::from_secs(30)));
    world.flush_gpu_mirror(first.queue()).unwrap();
    assert_eq!(resident(&first_textures), vec![(3, 2)]);

    // The device is lost, and the files are gone with it.
    drop(first);
    std::fs::remove_dir_all(project.path().join("images")).unwrap();
    std::fs::remove_dir_all(project.path().join("meshes")).unwrap();

    let second = context().expect("a second device");
    let (handle, store, second_textures) = mirror(&second);
    world.attach_gpu_mirror(handle);
    world.flush_gpu_mirror(second.queue()).unwrap();
    assert_eq!(
        resident(&second_textures),
        vec![(3, 2)],
        "the bound image is uploaded to the new device"
    );
    let geometry = world
        .get::<StaticMeshComponent>(mesh)
        .unwrap()
        .vertices
        .len();
    assert!(geometry > 0);
    assert!(
        StaticMeshComponent::vertices_gpu_handle(&store, mesh.index()).is_some(),
        "the bound mesh is uploaded to the new device"
    );
    assert!(!world.asset_work_pending(), "nothing was loaded again");
}
