//! Per-material draw segments (Helio#330 part 3): an object is always drawn
//! with its own material's pipeline, even in the frames right after other
//! objects change material and move every later material's draws.
//!
//! Draws used to take their offsets from a range table the CPU reads back
//! frames late. When the layout shifted, a late table drew some groups with
//! a neighbouring material's pipeline until the readback caught up.

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

/// Two custom material graphs that only differ in their (fixed) colour, so a
/// pixel says which pipeline drew it.
const RED_GRAPH: u64 = 0x5e9_0000_0000_0001;
const GREEN_GRAPH: u64 = 0x5e9_0000_0000_0002;

fn register_graphs() {
    helio_mats::register_graph_source(
        RED_GRAPH,
        "    albedo = vec4<f32>(0.0, 0.0, 0.0, 1.0);\n    emissive = vec3<f32>(8.0, 0.0, 0.0);\n"
            .to_string(),
    );
    helio_mats::register_graph_source(
        GREEN_GRAPH,
        "    albedo = vec4<f32>(0.0, 0.0, 0.0, 1.0);\n    emissive = vec3<f32>(0.0, 8.0, 0.0);\n"
            .to_string(),
    );
}

/// The bits of `object_batch.wgsl`'s sort key a graph hash contributes
/// (`compute_sort_key`): draws sort by class, then by this.
fn sort_bucket(graph: u64) -> u32 {
    let (lo, hi) = (graph as u32, (graph >> 32) as u32);
    let mut h = lo ^ hi.wrapping_mul(0x9E3779B1);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45D9F3B);
    h ^= h >> 13;
    h & 0xFFF
}

/// Points `object` at `graph`, keeping everything else.
fn set_graph(scene_db: &mut SceneDb, object: Entity, graph: u64) {
    let mut row = *scene_db.world.get::<StaticObjectComponent>(object).unwrap();
    row.material_class = helio_mats::MATERIAL_CLASS_CUSTOM;
    row.graph_hash_lo = graph as u32;
    row.graph_hash_hi = (graph >> 32) as u32;
    scene_db.world.insert(object, row);
}

/// A box with its own mesh (so it is its own draw group) drawn with `graph`.
fn spawn_box(scene_db: &mut SceneDb, material: Entity, translation: Vec3, graph: u64) -> Entity {
    let mesh = spawn_mesh(&mut scene_db.world, box_mesh([0.0; 3], [0.5; 3]));
    let object = spawn_object(
        &mut scene_db.world,
        mesh,
        material,
        Mat4::from_translation(translation),
        1.0,
    )
    .unwrap();
    set_graph(scene_db, object, graph);
    object
}

#[test]
fn objects_keep_their_material_while_other_objects_change_theirs() {
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
    register_graphs();

    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    let material = spawn_material(
        &mut scene_db.world,
        make_material([1.0; 4], 1.0, 0.0, [0.0; 3], 0.0),
    );
    // The object under test sits in the middle of the view with the graph
    // that sorts last. Movers behind the camera (culled, but their draw
    // groups still sit in the material ranges) start with the graph that
    // sorts first, so the object's draw starts after theirs. When they switch
    // to the object's graph its draw moves to the head of the buffer, where a
    // late range table still has the first graph's draws.
    assert_ne!(sort_bucket(RED_GRAPH), sort_bucket(GREEN_GRAPH));
    let red_sorts_last = sort_bucket(RED_GRAPH) > sort_bucket(GREEN_GRAPH);
    let (late, early) = if red_sorts_last {
        (RED_GRAPH, GREEN_GRAPH)
    } else {
        (GREEN_GRAPH, RED_GRAPH)
    };
    // Which channel each graph lights up.
    let (own, other) = if red_sorts_last { (0, 1) } else { (1, 0) };
    spawn_box(&mut scene_db, material, Vec3::ZERO, late);
    let movers: Vec<Entity> = (0..4)
        .map(|i| {
            spawn_box(
                &mut scene_db,
                material,
                Vec3::new(i as f32 * 2.0, 0.0, 20.0),
                early,
            )
        })
        .collect();

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
    let camera = Camera::perspective_look_at(
        Vec3::new(0.0, 0.0, 8.0),
        Vec3::ZERO,
        Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        1.0,
        0.1,
        100.0,
    );
    let mut render = |scene_db: &SceneDb| -> [u8; 3] {
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
        let i = (((SIZE / 2) * SIZE + SIZE / 2) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };

    // Both materials are known once the first readback lands.
    let mut frame = 0;
    for _ in 0..12 {
        render(&scene_db);
        frame += 1;
    }
    let pixel = render(&scene_db);
    assert!(
        pixel[own] > 100 && pixel[other] < 30,
        "the object is not drawn with its own graph: {pixel:?}"
    );

    // Move the movers between the two graphs, shifting where the object's
    // draw lies, and check every single frame.
    for round in 0..6 {
        let graph = if round % 2 == 0 { late } else { early };
        for &mover in &movers {
            set_graph(&mut scene_db, mover, graph);
        }
        for _ in 0..6 {
            let pixel = render(&scene_db);
            frame += 1;
            assert!(
                pixel[other] < 30,
                "frame {frame}: the object was drawn with the other graph's pipeline: {pixel:?}"
            );
            assert!(
                pixel[own] > 100,
                "frame {frame}: the object is missing: {pixel:?}"
            );
        }
    }
}
