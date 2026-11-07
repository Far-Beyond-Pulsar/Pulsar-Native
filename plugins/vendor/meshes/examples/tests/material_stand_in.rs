//! Without the materials plugin a mesh draws with the stand-in material:
//! default grey PBR (`helio_mats::MaterialRow::STAND_IN`), as the lights'
//! absence draws the G-buffer unlit.
//!
//! The same box is drawn twice: once over Helio's deferred preset with the
//! meshes' and lights' features only (no material texture table, no
//! material rows: the drawer binds `helio_mats::MaterialStandIn`), and once
//! with the materials plugin's feature and an authored row equal to the
//! stand-in. The two frames must agree, and show the box.

#[path = "../v3_demo_common.rs"]
#[allow(dead_code)]
mod v3_demo_common;

use glam::{Mat4, Vec3};
use helio::{Camera, RendererBuilder, RendererConfig};
use helio_mesh_host::{MeshComponent, StaticObjectComponent};
use std::sync::Arc;
use v3_demo_common::*;

const SIZE: u32 = 64;

/// A scene with the meshes' and the lights' columns, and the materials'
/// row column only when `materials`.
fn scene(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    materials: bool,
) -> pulsar_scenedb::SceneDb {
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
    StaticObjectComponent::register_gpu_columns_growable(&mut store, 64, device);
    helio_light_host::LightComponent::register_gpu_columns_growable(
        &mut store,
        helio_light_api::MAX_LIGHTS,
        device,
    );
    if materials {
        helio_mats::host_rows::MaterialComponent::register_gpu_columns_growable(
            &mut store, 64, device,
        );
    }
    let mirror = pulsar_scenedb::gpu::GpuMirrorHandle::new(Arc::new(store), queue.clone());
    scene_db.world.attach_gpu_mirror(mirror);
    scene_db
}

/// The centre pixel of one settled frame of a box whose object names
/// material row `material_slot`, drawn with `graph`.
fn draw(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    mut scene_db: pulsar_scenedb::SceneDb,
    material_slot: u32,
    graph: helio_presets::PresetGraph,
) -> [u8; 4] {
    let mesh = spawn_mesh(&mut scene_db.world, box_mesh([0.0; 3], [0.5; 3]));
    flush_scene_db(&scene_db, queue);
    let mirror = scene_db.world.gpu_mirror().cloned().unwrap();
    let vertices = MeshComponent::vertices_gpu_handle(mirror.store(), mesh.index()).unwrap();
    let indices = MeshComponent::indices_gpu_handle(mirror.store(), mesh.index()).unwrap();
    let object = scene_db.world.spawn();
    scene_db.world.insert(
        object,
        StaticObjectComponent::from(helio_mesh_api::StaticObjectRow::new(
            mesh.index(),
            mesh.generation().wrapping_add(1),
            material_slot,
            1,
            Mat4::IDENTITY,
            [0.0, 0.0, 0.0, 1.0],
            indices.count,
            indices.offset,
            vertices.offset as i32,
            helio_mats::MATERIAL_CLASS_DEFAULT,
            0,
            0,
        )),
    );

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = RendererBuilder::new(
        RendererConfig::new(SIZE, SIZE, format).with_render_scale(1.0),
        scene_db_handle(&scene_db),
    )
    .with_external_device()
    .with_graph(graph)
    .build(device.clone(), queue.clone(), SIZE, SIZE, format);
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.set_feature_live(helio::AmbientLight {
        color: [1.0; 3],
        intensity: 1.0,
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let camera = Camera::perspective_look_at(
        Vec3::new(1.5, 1.2, 3.0),
        Vec3::ZERO,
        Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        1.0,
        0.1,
        100.0,
    );
    // Settle: the object batch's readbacks land a few frames in.
    for _ in 0..12 {
        flush_scene_db(&scene_db, queue);
        renderer.render(&camera, &view).unwrap();
    }
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (SIZE * SIZE * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: None,
            },
        },
        target.size(),
    );
    queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let pixels = staging.slice(..).get_mapped_range().unwrap().to_vec();
    let i = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn without_the_materials_a_mesh_draws_the_grey_stand_in() {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(
            adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY,
        ),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(adapter.features()),
        ..Default::default()
    }))
    .expect("device");
    let (device, queue) = (Arc::new(device), Arc::new(queue));

    // No materials plugin: the meshes' and lights' features alone, no rows.
    let without = draw(
        &device,
        &queue,
        scene(&device, &queue, false),
        7,
        helio_presets::Preset::Deferred
            .with_features(&helio_mesh_host::features().extended(&helio_light_host::features())),
    );

    // The materials plugin, and an authored row that is the stand-in.
    let mut scene_db = scene(&device, &queue, true);
    let material = spawn_material(
        &mut scene_db.world,
        helio_mats::host_rows::MaterialComponent::from(helio_mats::MaterialRow::STAND_IN),
    );
    let with = draw(
        &device,
        &queue,
        scene_db,
        material.index(),
        helio_mesh_host::presets::preset(helio_presets::Preset::Deferred),
    );

    assert!(
        without[..3].iter().any(|&c| c > 8),
        "the box is drawn without the materials: {without:?}"
    );
    for c in 0..3 {
        assert!(
            without[c].abs_diff(with[c]) <= 1,
            "the stand-in is default grey PBR: {without:?} vs the authored stand-in row {with:?}"
        );
    }
    // Grey: no channel stands out.
    assert!(without[0].abs_diff(without[1]) <= 2 && without[1].abs_diff(without[2]) <= 2);
}
