//! Tiled light culling in the default (editor) graph must cover every light
//! SceneDB holds, not a fixed 64, and must re-cull when lights move while the
//! camera stays still.

#[path = "../v3_demo_common.rs"]
#[allow(dead_code)]
mod v3_demo_common;

use glam::{Mat4, Vec3};
use helio::{Camera, RendererBuilder, RendererConfig};
use helio_pass_light_cull::{LightCullPass, MAX_LIGHTS_PER_TILE};
use std::collections::BTreeSet;
use std::sync::Arc;
use v3_demo_common::*;

const WIDTH: u32 = 256;
const HEIGHT: u32 = 144;
const LIGHTS: usize = 100;

fn read(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer) -> Vec<u32> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("light cull readback"),
        size: source.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &staging, 0, source.size());
    queue.submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let words = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
    staging.unmap();
    words
}

/// Light rows referenced by any tile.
fn culled_rows(device: &wgpu::Device, queue: &wgpu::Queue, pass: &LightCullPass) -> BTreeSet<u32> {
    let counts = read(device, queue, &pass.tile_light_counts);
    let lists = read(device, queue, &pass.tile_light_lists);
    let mut rows = BTreeSet::new();
    for (tile, &count) in counts.iter().enumerate() {
        let start = tile * MAX_LIGHTS_PER_TILE as usize;
        rows.extend(&lists[start..start + count.min(MAX_LIGHTS_PER_TILE) as usize]);
    }
    rows
}

#[test]
fn culling_covers_grown_light_buffers_and_follows_moving_lights() {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        // Ray queries are not needed here, and some software drivers cannot
        // compile ray-query pipelines.
        required_features: helio::required_wgpu_features(
            adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY,
        ),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(adapter.features()),
        ..Default::default()
    }))
    .expect("device");
    let (device, queue) = (Arc::new(device), Arc::new(queue));

    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    let world = &mut scene_db.world;
    let material = spawn_material(world, make_material([0.6; 4], 0.8, 0.0, [0.0; 3], 0.0));
    let mesh = spawn_mesh(world, box_mesh([0.0; 3], [12.0, 0.1, 12.0]));
    spawn_object(world, mesh, material, Mat4::IDENTITY, 17.0).unwrap();
    let lights: Vec<_> = (0..LIGHTS)
        .map(|i| {
            let position = [
                (i % 10) as f32 * 2.2 - 10.0,
                0.6,
                (i / 10) as f32 * 2.2 - 10.0,
            ];
            (
                spawn_light(world, point_light(position, [1.0; 3], 5.0, 1.5)),
                position,
            )
        })
        .collect();

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = RendererBuilder::new(
        RendererConfig::new(WIDTH, HEIGHT, format).with_render_scale(1.0),
        scene_db_handle(&scene_db),
    )
    .with_external_device()
    .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
    .build(device.clone(), queue.clone(), WIDTH, HEIGHT, format);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let camera = Camera::perspective_look_at(
        Vec3::new(0.0, 22.0, 0.1),
        Vec3::ZERO,
        Vec3::Y,
        std::f32::consts::FRAC_PI_2,
        WIDTH as f32 / HEIGHT as f32,
        0.1,
        100.0,
    );
    let mut render = |scene_db: &pulsar_scenedb::SceneDb, renderer: &mut helio::Renderer| {
        for _ in 0..3 {
            flush_scene_db(scene_db, &queue);
            renderer.render(&camera, &view).unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    };

    render(&scene_db, &mut renderer);
    let light_rows: BTreeSet<u32> = lights.iter().map(|(entity, _)| entity.index()).collect();
    let culled = culled_rows(
        &device,
        &queue,
        renderer.find_pass::<LightCullPass>().unwrap(),
    );
    assert!(
        culled.is_subset(&light_rows),
        "culled rows that hold no light: {culled:?}"
    );
    let beyond_initial_capacity = light_rows.iter().filter(|&&row| row >= 64).count();
    assert!(
        beyond_initial_capacity > 0,
        "test must place lights past row 64"
    );
    assert_eq!(
        culled.iter().filter(|&&row| row >= 64).count(),
        beyond_initial_capacity,
        "lights stored past the buffer's initial 64 rows were not culled"
    );

    // Move every light far out of view without touching the camera.
    for &(entity, position) in &lights {
        update_light(
            &mut scene_db.world,
            entity,
            point_light([position[0], 500.0, position[2]], [1.0; 3], 5.0, 1.5),
        );
    }
    render(&scene_db, &mut renderer);
    let culled = culled_rows(
        &device,
        &queue,
        renderer.find_pass::<LightCullPass>().unwrap(),
    );
    assert!(
        culled.is_empty(),
        "moved lights kept stale tile assignments: {culled:?}"
    );
}
