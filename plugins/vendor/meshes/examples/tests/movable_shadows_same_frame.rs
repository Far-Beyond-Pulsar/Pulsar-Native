//! Movable shadow casters follow this frame's object set: a movable object
//! spawned between frames casts its shadow in the very next frame, and a
//! despawned one's shadow is gone in it.
//!
//! The shadow passes used to size their work from Object Batch's movable
//! caster count as read back on the CPU, two or more frames late: a new
//! caster's shadow appeared late, and a removed one's shadow lingered.

#[path = "../v3_demo_common.rs"]
#[allow(dead_code)]
mod v3_demo_common;

use glam::{Mat4, Vec3};
use helio::{Camera, Movability, RendererBuilder, RendererConfig};
use pulsar_scenedb::SceneDb;
use std::sync::Arc;
use v3_demo_common::*;

const SIZE: u32 = 64;

#[test]
fn spawned_and_despawned_movable_casters_shadow_in_the_next_frame() {
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
    let white = spawn_material(
        &mut scene_db.world,
        make_material([1.0; 4], 1.0, 0.0, [0.0; 3], 0.0),
    );
    // A static ground plane, lit by a shadowed spot light off to the left,
    // so a box above the origin throws its shadow to the right of it.
    let ground = spawn_mesh(&mut scene_db.world, plane_mesh([0.0; 3], 8.0));
    spawn_object(&mut scene_db.world, ground, white, Mat4::IDENTITY, 12.0).unwrap();
    let mut light = spot_light(
        [-4.0, 6.0, 0.0],
        Vec3::new(4.0, -6.0, 0.0).normalize().to_array(),
        [1.0; 3],
        60.0,
        30.0,
        0.6,
        0.9,
    );
    light.shadow_index = SHADOW_BASES[0];
    spawn_light(&mut scene_db.world, light);
    let box_mesh = spawn_mesh(&mut scene_db.world, cube_mesh([0.0; 3], 0.5));

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
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (SIZE * SIZE * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Straight down at the ground, +Z up the image.
    let camera = Camera::perspective_look_at(
        Vec3::new(0.0, 10.0, 0.0),
        Vec3::ZERO,
        Vec3::Z,
        std::f32::consts::FRAC_PI_4,
        1.0,
        0.1,
        100.0,
    );
    // Renders one frame; returns the brightness of the ground where the
    // box's shadow falls (the light is on the left, so it falls to the
    // right of the box: the image's x runs the other way, to the left).
    let mut render = |scene_db: &SceneDb| -> u32 {
        flush_scene_db(scene_db, &queue);
        renderer.render(&camera, &view).unwrap();
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
        let i = (((SIZE / 2) * SIZE + SIZE * 5 / 16) * 4) as usize;
        pixels[i] as u32 + pixels[i + 1] as u32 + pixels[i + 2] as u32
    };

    // Let the readback, the shadow atlas and its scheduler settle.
    for _ in 0..12 {
        render(&scene_db);
    }
    let lit = render(&scene_db);
    assert!(lit > 100, "the ground is not lit to begin with: {lit}");

    for round in 0..3 {
        let caster = spawn_object_with_movability(
            &mut scene_db.world,
            box_mesh,
            white,
            Mat4::from_translation(Vec3::new(0.0, 1.5, 0.0)),
            0.9,
            Some(Movability::Movable),
        )
        .unwrap();
        let shadowed = render(&scene_db);
        assert!(
            shadowed * 3 < lit,
            "round {round}: the spawned caster's shadow is missing in its first frame: \
             {shadowed} (lit {lit})"
        );
        for _ in 0..4 {
            render(&scene_db);
        }

        scene_db.world.despawn(caster);
        let after = render(&scene_db);
        assert!(
            after * 10 > lit * 9,
            "round {round}: the despawned caster's shadow is still there in the next frame: \
             {after} (lit {lit})"
        );
        for _ in 0..4 {
            render(&scene_db);
        }
    }
}
