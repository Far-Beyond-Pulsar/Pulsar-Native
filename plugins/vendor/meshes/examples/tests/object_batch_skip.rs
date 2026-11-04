//! The object-batch pass skips its pipeline while nothing it reads changes.
//! A move after a run of skipped frames must still reach the image.

#[path = "../v3_demo_common.rs"]
#[allow(dead_code)]
mod v3_demo_common;

use glam::{Mat4, Vec3};
use helio::{Camera, RendererBuilder, RendererConfig};
use std::sync::Arc;
use v3_demo_common::*;

const SIZE: u32 = 64;

#[test]
fn moving_an_object_after_idle_frames_updates_the_image() {
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

    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    let material = spawn_material(
        &mut scene_db.world,
        make_material([1.0; 4], 1.0, 0.0, [1.0, 1.0, 1.0], 20.0),
    );
    let mesh = spawn_mesh(&mut scene_db.world, cube_mesh([0.0; 3], 1.0));
    let object = spawn_object(&mut scene_db.world, mesh, material, Mat4::IDENTITY, 1.8).unwrap();

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = RendererBuilder::new(
        RendererConfig::new(SIZE, SIZE, format).with_render_scale(1.0),
        scene_db_handle(&scene_db),
    )
    .with_external_device()
    .with_pass_build_context(Box::new(
        helio_mesh_host::presets::deferred,
    ))
    .build(device.clone(), queue.clone(), SIZE, SIZE, format);
    renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);
    renderer.set_feature_live(helio::AmbientLight {
        color: [0.0; 3],
        intensity: 0.0,
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
        Vec3::new(0.0, 0.0, 8.0),
        Vec3::ZERO,
        Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        1.0,
        0.1,
        100.0,
    );
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (SIZE * SIZE * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Renders `frames` frames and returns the brightness of the centre pixel
    // and of a pixel a quarter of the way in from the right edge.
    let render = |renderer: &mut helio::Renderer,
                  scene_db: &pulsar_scenedb::SceneDb,
                  frames: usize|
     -> (u32, u32) {
        for _ in 0..frames {
            flush_scene_db(scene_db, &queue);
            renderer.render(&camera, &view).unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
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
        staging.unmap();
        let brightness = |x: u32, y: u32| {
            let i = ((y * SIZE + x) * 4) as usize;
            pixels[i] as u32 + pixels[i + 1] as u32 + pixels[i + 2] as u32
        };
        (
            brightness(SIZE / 2, SIZE / 2),
            brightness(SIZE * 3 / 4, SIZE / 2),
        )
    };

    // Many frames with nothing changing: the batch settles and skips.
    let (centre, side) = render(&mut renderer, &scene_db, 12);
    assert!(
        centre > side + 60,
        "object not drawn at the centre: centre {centre}, side {side}"
    );

    // Move it right, then leave it alone again.
    update_object_transform(
        &mut scene_db.world,
        &mut renderer,
        object,
        Mat4::from_translation(Vec3::new(1.65, 0.0, 0.0)),
    )
    .unwrap();
    let (centre, side) = render(&mut renderer, &scene_db, 12);
    assert!(
        side > centre + 60,
        "the move after idle frames never reached the image: centre {centre}, side {side}"
    );

    // Despawning removes it from the image.
    despawn_object(&mut scene_db.world, &mut renderer, object).unwrap();
    let (centre, side) = render(&mut renderer, &scene_db, 6);
    assert!(
        side < 30 && centre < 30,
        "despawned object still drawn: centre {centre}, side {side}"
    );
}
