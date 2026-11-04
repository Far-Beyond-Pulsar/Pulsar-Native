//! World-space GI in a mesh scene's default graph (no voxel terrain): with
//! ray queries the probes are traced by the scene's meshes, and update only
//! on frames whose renderer has a TLAS (`SceneDbRayTracing::partial`
//! published); without ray queries there is no GI, the sky's ambient as
//! before.

use helio_components::LightComponent;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    RendererBuilder, RendererConfig,
};
use helio_mesh_host::ray_tracing::SceneDbRayTracing;
use helio_components::{MaterialComponent};
use helio_mesh_host::{MeshComponent, StaticObjectComponent};
use helio_pass_gi::{GiMeshTracePass, GiPass, PROBES, PROBE_WORDS};

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
    helio_components::LightComponent::register_gpu_columns_growable(
        &mut store,
        helio_pass_light_cull::MAX_LIGHTS,
        device,
    );
    scene_db
        .world
        .attach_gpu_mirror(pulsar_scenedb::gpu::GpuMirrorHandle::new(
            Arc::new(store),
            queue.clone(),
        ));
    scene_db
}

/// An axis-aligned box of half extent `half` at `centre` in `colour`.
fn spawn_box(world: &mut pulsar_scenedb::World, centre: Vec3, half: Vec3, colour: [f32; 3]) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let n = Vec3::from_array(std::array::from_fn(|k| if k == axis { sign } else { 0.0 }));
            let (u, v) = (
                Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 1) % 3))),
                Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 2) % 3))),
            );
            let (u, v) = if sign > 0.0 { (u, v) } else { (v, u) };
            let base = vertices.len() as u32;
            for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = (n + u * a + v * b) * half;
                vertices.push(helio_mesh_api::PackedVertex::from_components(
                    p.to_array(),
                    n.to_array(),
                    [0.0; 2],
                    u.to_array(),
                    1.0,
                ));
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    let mesh = world.spawn();
    world.insert(mesh, MeshComponent { vertices, indices });
    let material = world.spawn();
    let row = MaterialComponent::from(helio_mats::MaterialRow::new(
        [colour[0], colour[1], colour[2], 1.0],
        0.8,
        0.0,
        [0.0; 3],
        0.0,
    ));
    world.insert(material, row);
    let store = world.gpu_mirror().cloned().unwrap();
    let vertex_range = MeshComponent::vertices_gpu_handle(store.store(), mesh.index()).unwrap();
    let index_range = MeshComponent::indices_gpu_handle(store.store(), mesh.index()).unwrap();
    let transform = Mat4::from_translation(centre);
    let object = StaticObjectComponent::from(helio_mesh_api::StaticObjectRow::new(
        mesh.index(),
        mesh.generation().wrapping_add(1),
        material.index(),
        material.generation().wrapping_add(1),
        transform,
        [centre.x, centre.y, centre.z, half.length()],
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

/// A room (floor, walls, ceiling: boxes) with a pillar and a point light.
fn room(world: &mut pulsar_scenedb::World) {
    let light = world.spawn();
    world.insert(
        light,
        LightComponent::from(helio_pass_light_cull::LightRow::from(
            helio_pass_light_cull::GpuLight {
                position_range: [0.0, 2.5, 0.0, 12.0],
                color_intensity: [1.0, 0.9, 0.8, 20.0],
                shadow_index: u32::MAX,
                light_type: 1,
                ..Default::default()
            },
        )),
    );
    let white = [0.7; 3];
    spawn_box(
        world,
        Vec3::new(0.0, -0.25, 0.0),
        Vec3::new(5.0, 0.25, 5.0),
        white,
    );
    spawn_box(
        world,
        Vec3::new(0.0, 4.25, 0.0),
        Vec3::new(5.0, 0.25, 5.0),
        white,
    );
    spawn_box(
        world,
        Vec3::new(-5.25, 2.0, 0.0),
        Vec3::new(0.25, 2.5, 5.0),
        [0.7, 0.1, 0.1],
    );
    spawn_box(
        world,
        Vec3::new(5.25, 2.0, 0.0),
        Vec3::new(0.25, 2.5, 5.0),
        [0.1, 0.7, 0.1],
    );
    spawn_box(
        world,
        Vec3::new(0.0, 2.0, -5.25),
        Vec3::new(5.0, 2.5, 0.25),
        white,
    );
    spawn_box(
        world,
        Vec3::new(0.0, 2.0, 5.25),
        Vec3::new(5.0, 2.5, 0.25),
        white,
    );
    spawn_box(
        world,
        Vec3::new(1.5, 1.0, -1.0),
        Vec3::new(0.4, 1.0, 0.4),
        white,
    );
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u32> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec()
}

/// The probes updated at least once, and of those the ones with light.
fn probes_lit(words: &[u32]) -> (usize, usize) {
    let mut updated = 0;
    let mut lit = 0;
    for index in 0..PROBES as usize {
        let at = index * PROBE_WORDS as usize;
        if words[at + 6] >> 8 == 0 {
            continue;
        }
        updated += 1;
        // The DC term's red and green (half floats): any light at all.
        if words[at] & 0x7fff_7fff != 0 {
            lit += 1;
        }
    }
    (updated, lit)
}

fn adapter() -> Option<wgpu::Adapter> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()
}

#[test]
fn a_mesh_scene_s_probes_trace_its_meshes_on_frames_with_a_tlas() {
    let Some(adapter) = adapter() else { return };
    let features = required_wgpu_features(helio::usable_adapter_features(&adapter));
    if !features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
        eprintln!("no ray queries: skipped");
        return;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        required_limits: required_wgpu_limits(adapter.limits()),
        experimental_features: required_experimental_features(features),
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) = (Arc::new(device), Arc::new(queue));
    let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut scene = scene_db(&device, &queue);
    room(&mut scene.world);
    let config = RendererConfig::new(64, 64, wgpu::TextureFormat::Rgba8Unorm);
    let mut renderer =
        RendererBuilder::new(config.clone(), scene.world.gpu_mirror().cloned().unwrap())
            .with_graph(helio_mesh_host::presets::with(helio_presets::Preset::Deferred, &helio_feature_api::FeatureSet::new().with(helio_pass_gi::GiFeature),
            ))
            .with_external_device()
            .build(
                device.clone(),
                queue.clone(),
                config.width,
                config.height,
                config.surface_format,
            );
    assert!(
        renderer.find_pass::<GiMeshTracePass>().is_some(),
        "no mesh tracer in the graph"
    );
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let camera = Camera::perspective_look_at(
        Vec3::new(-2.0, 1.7, 3.0),
        Vec3::new(1.0, 1.0, -1.0),
        Vec3::Y,
        1.0,
        1.0,
        0.1,
        100.0,
    );
    let probes = |renderer: &helio::Renderer| {
        read(
            &device,
            &queue,
            renderer.find_pass::<GiPass>().unwrap().buffers()[1],
        )
    };
    // No TLAS published: the probes wait.
    for _ in 0..3 {
        scene.world.flush_gpu_mirror(&queue);
        renderer.render(&camera, &view).unwrap();
    }
    assert_eq!(
        probes_lit(&probes(&renderer)),
        (0, 0),
        "probes updated without a TLAS"
    );
    let mut rays = SceneDbRayTracing::partial(device.clone(), queue.clone());
    for _ in 0..40 {
        scene.world.flush_gpu_mirror(&queue);
        rays.prepare(&scene.world).unwrap();
        rays.publish(&mut renderer);
        renderer.render(&camera, &view).unwrap();
    }
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let (updated, lit) = probes_lit(&probes(&renderer));
    // 40 frames of 2720 updates, the room's light bounced into most of the
    // finest probes inside it.
    assert!(updated > 40_000, "{updated} probes updated");
    assert!(lit > 1000, "{lit} probes lit of {updated}");
    let error = pollster::block_on(errors.pop());
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn without_ray_queries_a_mesh_scene_has_no_gi() {
    let Some(adapter) = adapter() else { return };
    let features =
        required_wgpu_features(adapter.features()) - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        required_limits: required_wgpu_limits(adapter.limits()),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) = (Arc::new(device), Arc::new(queue));
    assert!(GiMeshTracePass::new(&device).is_none());
    let mut scene = scene_db(&device, &queue);
    room(&mut scene.world);
    let config = RendererConfig::new(32, 32, wgpu::TextureFormat::Rgba8Unorm);
    let mut renderer =
        RendererBuilder::new(config.clone(), scene.world.gpu_mirror().cloned().unwrap())
            .with_graph(helio_mesh_host::presets::with(helio_presets::Preset::Deferred, &helio_feature_api::FeatureSet::new().with(helio_pass_gi::GiFeature),
            ))
            .with_external_device()
            .build(
                device.clone(),
                queue.clone(),
                config.width,
                config.height,
                config.surface_format,
            );
    assert!(
        renderer.find_pass::<GiPass>().is_none(),
        "GI without a tracer"
    );
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    scene.world.flush_gpu_mirror(&queue);
    let camera = Camera::perspective_look_at(
        Vec3::new(-2.0, 1.7, 3.0),
        Vec3::new(1.0, 1.0, -1.0),
        Vec3::Y,
        1.0,
        1.0,
        0.1,
        100.0,
    );
    renderer
        .render(&camera, &target.create_view(&Default::default()))
        .unwrap();
}
