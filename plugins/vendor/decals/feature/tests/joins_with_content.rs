//! The decal drawer follows the scene in Helio's default graph plus this
//! plugin's feature: in a mesh room whose decal column is registered but
//! empty (as editors register it up front) the decal pass is dormant, and a
//! decal row wakes it once the row has been read back. Needs a GPU adapter
//! (lavapipe works); skips without one.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use helio::{
    required_wgpu_features, required_wgpu_limits, Camera, RendererBuilder, RendererConfig,
};
use helio_components::{MaterialComponent};
use helio_mesh_host::{MeshComponent, StaticObjectComponent};
use pulsar_scenedb_derive::SceneStore;

/// A decal pass row as a SceneDB component, as the plugin's join writes it.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "decals")]
struct DecalRowComponent {
    #[gpu]
    transform: [f32; 16],
    #[gpu]
    color: [f32; 4],
    #[gpu]
    textures: [u32; 4],
    #[gpu]
    blend_type_fade: [u32; 4],
    #[gpu]
    age_adapt_pad: [u32; 4],
}

const _: () = assert!(
    std::mem::size_of::<DecalRowComponent>() == std::mem::size_of::<helio_pass_decal::DecalRow>()
);

fn scene_db(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> pulsar_scenedb::SceneDb {
    let mut scene_db = pulsar_scenedb::SceneDb::new();
    let ctx = pulsar_scenedb::gpu::EngineGpuContext::new(device.clone(), queue.clone());
    let mut store = pulsar_scenedb::gpu::SceneGpuStore::new(
        &ctx,
        pulsar_scenedb::gpu::SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    MeshComponent::register_gpu_columns_growable(&mut store, 64, device);
    MaterialComponent::register_gpu_columns_growable(&mut store, 64, device);
    StaticObjectComponent::register_gpu_columns_growable(&mut store, 64, device);
    // Registered before any decal exists: the column alone must not wake
    // the decal pass.
    DecalRowComponent::register_gpu_columns_growable(&mut store, 64, device);
    scene_db
        .world
        .attach_gpu_mirror(pulsar_scenedb::gpu::GpuMirrorHandle::new(
            Arc::new(store),
            queue.clone(),
        ));
    scene_db
}

/// A floor slab.
fn floor(world: &mut pulsar_scenedb::World) {
    let half = Vec3::new(5.0, 0.25, 5.0);
    let mut vertices = Vec::new();
    for (a, b) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        vertices.push(helio_mesh_api::PackedVertex::from_components(
            [a * half.x, half.y, b * half.z],
            [0.0, 1.0, 0.0],
            [0.0; 2],
            [1.0, 0.0, 0.0],
            1.0,
        ));
    }
    let mesh = world.spawn();
    world.insert(
        mesh,
        MeshComponent {
            vertices,
            indices: vec![0, 2, 1, 0, 3, 2],
        },
    );
    let material = world.spawn();
    let row = MaterialComponent::from(helio_mats::MaterialRow::new(
        [0.7, 0.7, 0.7, 1.0],
        0.8,
        0.0,
        [0.0; 3],
        0.0,
    ));
    world.insert(material, row);
    let store = world.gpu_mirror().cloned().unwrap();
    let vertex_range = MeshComponent::vertices_gpu_handle(store.store(), mesh.index()).unwrap();
    let index_range = MeshComponent::indices_gpu_handle(store.store(), mesh.index()).unwrap();
    let object = StaticObjectComponent::from(helio_pass_gbuffer::StaticObjectRow::new(
        mesh.index(),
        mesh.generation().wrapping_add(1),
        material.index(),
        material.generation().wrapping_add(1),
        Mat4::IDENTITY,
        [0.0, 0.0, 0.0, half.length()],
        index_range.count,
        index_range.offset,
        vertex_range.offset as i32,
        row.material_class,
        0,
        0,
    ));
    let entity = world.spawn();
    world.insert(entity, object);
}

#[test]
fn the_decal_pass_joins_with_its_first_row() {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    })) else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: decal pass membership");
        return;
    };
    let features = required_wgpu_features(helio::usable_adapter_features(&adapter));
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        required_limits: required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) = (Arc::new(device), Arc::new(queue));
    let mut scene = scene_db(&device, &queue);
    floor(&mut scene.world);
    let config = RendererConfig::new(64, 48, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mut renderer =
        RendererBuilder::new(config.clone(), scene.world.gpu_mirror().cloned().unwrap())
            .with_graph(helio_mesh_host::presets::with(helio_presets::Preset::Deferred, &helio_feature_api::FeatureSet::new().with(helio_pass_decal::DecalFeature),
            ))
            .with_external_device()
            .build(
                device.clone(),
                queue.clone(),
                config.width,
                config.height,
                config.surface_format,
            );
    let camera = Camera::perspective_look_at(
        Vec3::new(0.0, 3.0, 6.0),
        Vec3::ZERO,
        Vec3::Y,
        1.0,
        64.0 / 48.0,
        0.1,
        100.0,
    );
    let view = device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 64,
                height: 48,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let frames =
        |renderer: &mut helio::Renderer, scene: &mut pulsar_scenedb::SceneDb, count| {
            for _ in 0..count {
                scene.world.flush_gpu_mirror(&queue);
                renderer.render(&camera, &view).unwrap();
                // Row readbacks complete between frames.
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            }
        };

    frames(&mut renderer, &mut scene, 4);
    assert!(
        !renderer.graph_members().contains(&"DecalApply"),
        "the decal pass runs without decals: {:?}",
        renderer.graph_members()
    );

    let mut row: helio_pass_decal::DecalRow = bytemuck::Zeroable::zeroed();
    row.transform = Mat4::from_scale(Vec3::splat(2.0)).to_cols_array();
    row.color = [1.0, 0.0, 0.0, 1.0];
    let decal = scene.world.spawn();
    scene.world.insert(
        decal,
        bytemuck::cast::<_, DecalRowComponent>(row),
    );
    frames(&mut renderer, &mut scene, 4);
    assert!(
        renderer.graph_members().contains(&"DecalApply"),
        "{:?}",
        renderer.graph_members()
    );
}
