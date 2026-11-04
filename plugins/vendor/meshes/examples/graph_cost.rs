//! What the default graph costs a scene: CPU per frame, GPU memory and
//! resize time, headless.
//!
//! `graph_cost <out dir> [room] [width height]`
//!
//! - `room`: `indoor_room`'s scene (a furnished room, three point lights).
//!
//! The `content` scene (the room plus water, particles and decals) is gone:
//! those kinds left Helio with their plugins (Pulsar `plugins/vendor/<kind>`),
//! whose own examples measure them.
//!
//! Renders 120 frames to settle, times `Renderer::render` over 300 more
//! (waiting for the GPU between frames, outside the timing), writes every
//! GPU allocation by label to `<out>/memory_<scene>.csv`, then resizes six
//! times between the size given and 80% of it and times each resize's frame
//! less a steady frame. Prints one `GRAPHCOST` line.

mod v3_demo_common;

use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use helio::{required_wgpu_features, required_wgpu_limits, Camera, RendererConfig};
use v3_demo_common::{
    box_mesh, build_default_renderer, flush_scene_db, make_material, new_scene_db_with_gpu_mirror,
    plane_mesh, point_light, spawn_light, spawn_material, spawn_mesh,
    spawn_object,
};

fn room(world: &mut pulsar_scenedb::World) {
    let mat = spawn_material(
        world,
        make_material([0.7, 0.68, 0.62, 1.0], 0.8, 0.0, [0.0, 0.0, 0.0], 0.0),
    );
    let mut place = |mesh, at: Vec3, radius| {
        let mesh = spawn_mesh(world, mesh);
        let _ = spawn_object(world, mesh, mat, Mat4::from_translation(at), radius);
    };
    place(plane_mesh([0.0, 0.0, 0.0], 4.0), Vec3::ZERO, 4.0);
    place(plane_mesh([0.0, 3.0, 0.0], 4.0), Vec3::ZERO, 4.0);
    place(
        box_mesh([0.0; 3], [4.0, 1.5, 0.05]),
        Vec3::new(0.0, 1.5, -4.0),
        4.0,
    );
    place(
        box_mesh([0.0; 3], [4.0, 1.5, 0.05]),
        Vec3::new(0.0, 1.5, 4.0),
        4.0,
    );
    place(
        box_mesh([0.0; 3], [0.05, 1.5, 4.0]),
        Vec3::new(4.0, 1.5, 0.0),
        4.0,
    );
    place(
        box_mesh([0.0; 3], [0.05, 1.5, 4.0]),
        Vec3::new(-4.0, 1.5, 0.0),
        4.0,
    );
    place(
        box_mesh([0.0; 3], [0.8, 0.4, 0.5]),
        Vec3::new(1.5, 0.4, 1.5),
        0.8,
    );
    place(
        box_mesh([0.0; 3], [0.3, 1.0, 1.2]),
        Vec3::new(-3.4, 1.0, -2.5),
        1.0,
    );
    place(
        box_mesh([0.0; 3], [1.2, 0.35, 0.5]),
        Vec3::new(-1.5, 0.35, 2.5),
        1.2,
    );
    spawn_light(
        world,
        point_light([0.0, 2.85, 0.0], [1.0, 0.85, 0.6], 4.0, 7.0),
    );
    spawn_light(
        world,
        point_light([-2.5, 0.9, -2.5], [1.0, 0.55, 0.2], 2.5, 5.0),
    );
    spawn_light(
        world,
        point_light([2.5, 0.9, 2.5], [1.0, 0.75, 0.35], 2.0, 4.5),
    );
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).map_or("graph_cost_out", String::as_str));
    let scene_name = args.get(2).map_or("room", String::as_str).to_owned();
    let width: u32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(1600);
    let height: u32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(900);
    std::fs::create_dir_all(&out).expect("out dir");

    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    let features = required_wgpu_features(helio::usable_adapter_features(&adapter));
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("graph_cost"),
        required_features: features,
        required_limits: required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .expect("device");
    let (device, queue) = (Arc::new(device), Arc::new(queue));

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
    room(&mut scene_db.world);
    let config = RendererConfig::new(width, height, format);
    let mut renderer = build_default_renderer(&scene_db, device.clone(), queue.clone(), config);
    renderer.set_feature_live(helio::AmbientLight {
            color: [1.0, 0.95, 0.85],
            intensity: 0.05,
        });

    let target = |w: u32, h: u32| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("graph_cost target"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let camera = |w: u32, h: u32| {
        Camera::perspective_look_at(
            Vec3::new(0.0, 1.6, 3.5),
            Vec3::new(0.0, 0.8, 0.0),
            Vec3::Y,
            1.1,
            w as f32 / h as f32,
            0.05,
            200.0,
        )
    };
    let wait = || {
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("frame completes");
    };
    // One frame: the scene flush, then `render` timed.
    let frame = |renderer: &mut helio::Renderer, view: &wgpu::TextureView, w, h| -> f64 {
        flush_scene_db(&scene_db, &queue);
        let started = Instant::now();
        renderer.render(&camera(w, h), view).expect("render");
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        wait();
        ms
    };

    let view = target(width, height);
    for _ in 0..120 {
        frame(&mut renderer, &view, width, height);
    }
    let mut times: Vec<f64> = (0..300)
        .map(|_| frame(&mut renderer, &view, width, height))
        .collect();
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    times.sort_by(f64::total_cmp);
    let p50 = percentile(&times, 0.5);
    let p95 = percentile(&times, 0.95);

    let (mut mem_mb, mut allocations) = (0.0, 0);
    if let Some(report) = device.generate_allocator_report() {
        let mut by_label: std::collections::HashMap<&str, (u64, usize)> = Default::default();
        for a in &report.allocations {
            let entry = by_label.entry(a.name.as_str()).or_default();
            entry.0 += a.size;
            entry.1 += 1;
        }
        let mut rows: Vec<_> = by_label.into_iter().collect();
        rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
        let mut csv = String::from("label,mb,count\n");
        for (label, (bytes, n)) in &rows {
            csv += &format!(
                "\"{}\",{:.2},{n}\n",
                label.replace('"', "'"),
                *bytes as f64 / 1048576.0
            );
        }
        std::fs::write(out.join(format!("memory_{scene_name}.csv")), csv).expect("csv");
        mem_mb = report.allocations.iter().map(|a| a.size).sum::<u64>() as f64 / 1048576.0;
        allocations = report.allocations.len();
    }

    // Resize: each resized frame less a steady frame.
    let small = ((width * 4 / 5).max(1), (height * 4 / 5).max(1));
    let mut resizes = Vec::new();
    for i in 0..6 {
        let (w, h) = if i % 2 == 0 { small } else { (width, height) };
        let view = target(w, h);
        let started = Instant::now();
        renderer.set_render_size(w, h);
        let ms = started.elapsed().as_secs_f64() * 1000.0 + frame(&mut renderer, &view, w, h);
        let steady = frame(&mut renderer, &view, w, h);
        resizes.push(ms - steady);
    }
    resizes.sort_by(f64::total_cmp);

    println!(
        "GRAPHCOST scene={scene_name} size={width}x{height} cpu_ms mean={mean:.3} p50={p50:.3} \
         p95={p95:.3} gpu_mem_mb={mem_mb:.1} allocations={allocations} resize_ms p50={:.2} \
         max={:.2}",
        percentile(&resizes, 0.5),
        resizes[resizes.len() - 1],
    );
}
