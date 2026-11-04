//! Culling follows this frame's object set: an object spawned between frames
//! is drawn in the very next frame, and a despawned one is gone in it.
//!
//! The frustum and occlusion culls used to size their dispatches from Object
//! Batch's draw count as read back on the CPU, two or more frames late. A new
//! draw group was then left unculled (so undrawn) until the readback caught
//! up, and a removed one kept being drawn from stale records.

#[path = "../v3_demo_common.rs"]
#[allow(dead_code)]
mod v3_demo_common;

use glam::{Mat4, Vec3};
use helio::{Camera, RendererBuilder, RendererConfig};
use helio_mesh_host::StaticObjectComponent;
use pulsar_scenedb::{Entity, SceneDb};
use std::sync::Arc;
use v3_demo_common::*;

const SIZE: u32 = 64;

/// A custom material graph with a fixed, bright red colour, so a pixel says
/// whether an object was drawn there.
const RED_GRAPH: u64 = 0xc011_0000_0000_0001;

/// A box with its own mesh (so it is its own draw group), drawn red.
fn spawn_box(scene_db: &mut SceneDb, material: Entity, translation: Vec3) -> Entity {
    let mesh = spawn_mesh(&mut scene_db.world, box_mesh([0.0; 3], [0.5; 3]));
    let object = spawn_object(
        &mut scene_db.world,
        mesh,
        material,
        Mat4::from_translation(translation),
        1.0,
    )
    .unwrap();
    let mut row = *scene_db.world.get::<StaticObjectComponent>(object).unwrap();
    row.material_class = helio_mats::MATERIAL_CLASS_CUSTOM;
    row.graph_hash_lo = RED_GRAPH as u32;
    row.graph_hash_hi = (RED_GRAPH >> 32) as u32;
    scene_db.world.insert(object, row);
    object
}

#[test]
fn spawned_and_despawned_objects_show_in_the_next_frame() {
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
    helio_mats::register_graph_source(
        RED_GRAPH,
        "    albedo = vec4<f32>(0.0, 0.0, 0.0, 1.0);\n    emissive = vec3<f32>(8.0, 0.0, 0.0);\n"
            .to_string(),
    );

    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    let material = spawn_material(
        &mut scene_db.world,
        make_material([1.0; 4], 1.0, 0.0, [0.0; 3], 0.0),
    );
    // Stays put on the left while the object under test comes and goes at
    // the centre. It also keeps the red material's draw segment alive, so
    // only culling decides what is drawn. Every frame must draw both: which
    // of the two a late group count leaves out depends on their sort order.
    spawn_box(&mut scene_db, material, Vec3::new(-2.0, 0.0, 0.0));

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = RendererBuilder::new(
        RendererConfig::new(SIZE, SIZE, format).with_render_scale(1.0),
        scene_db_handle(&scene_db),
    )
    .with_external_device()
    .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
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
    let camera = Camera::perspective_look_at(
        Vec3::new(0.0, 0.0, 8.0),
        Vec3::ZERO,
        Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        1.0,
        0.1,
        100.0,
    );
    // Renders one frame; returns the red channel at the centre and on the
    // left object.
    let mut render = |scene_db: &SceneDb| -> (u8, u8) {
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
        let red = |x: u32| pixels[(((SIZE / 2) * SIZE + x) * 4) as usize];
        (red(SIZE / 2), red(SIZE / 5))
    };

    // Let the readback, the draw segments and Hi-Z settle.
    for _ in 0..12 {
        render(&scene_db);
    }
    let (centre, left) = render(&scene_db);
    assert!(
        centre < 30 && left > 100,
        "expected only the left object to begin with: centre {centre}, left {left}"
    );

    for round in 0..3 {
        let object = spawn_box(&mut scene_db, material, Vec3::ZERO);
        let (centre, left) = render(&scene_db);
        assert!(
            centre > 100 && left > 100,
            "round {round}: the frame after a spawn did not draw both objects: \
             centre {centre}, left {left}"
        );
        for _ in 0..6 {
            render(&scene_db);
        }

        scene_db.world.despawn(object);
        let (centre, left) = render(&scene_db);
        assert!(
            centre < 30 && left > 100,
            "round {round}: the frame after a despawn did not draw just the left object: \
             centre {centre}, left {left}"
        );
        for _ in 0..6 {
            render(&scene_db);
        }
    }
}
