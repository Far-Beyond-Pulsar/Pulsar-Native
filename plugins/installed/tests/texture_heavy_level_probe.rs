//! TEMPORARY diagnostic probe (not for commit): loads a real project's level
//! headless through the editor's renderer, streaming its assets in as frames
//! run (the level is applied without waiting for assets, like the game and
//! editor do), and reports what its graph materials resolve to plus the frame
//! times while it streams: the worst frame, p99 and the assets still pending.
//! Run with PROBE_PROJECT and PROBE_LEVEL set; `PROBE_FRAME_BUDGET_MS` fails the
//! test when the worst frame past warm-up exceeds it.

use installed_plugins as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_backend::scene::RuntimeLevel;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};

const SIZE: u32 = 512;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[test]
fn probe_texture_heavy_level() {
    let (Ok(project), Ok(level)) = (std::env::var("PROBE_PROJECT"), std::env::var("PROBE_LEVEL"))
    else {
        eprintln!("PROBE_PROJECT/PROBE_LEVEL unset; skipping");
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .try_init();

    let mut descriptor = helio_core::wgpu_instance_descriptor();
    if std::env::var("PROBE_DX12").is_ok() {
        descriptor.backends = wgpu::Backends::DX12;
    }
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!(
        "PROBE adapter {:?} {:?}",
        adapter.get_info().name,
        adapter.get_info().backend
    );
    let features = helio::required_wgpu_features(adapter.features());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("probe"),
        required_features: features,
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .unwrap();
    device.on_uncaptured_error(Arc::new(|error| eprintln!("PROBE WGPU ERROR: {error}")));
    eprintln!(
        "PROBE table capacity {} binding_arrays {}",
        helio_core::texture_table_capacity(device.features(), &device.limits()),
        helio_core::texture_table_uses_binding_arrays(device.features())
    );
    let (device, queue) = (Arc::new(device), Arc::new(queue));

    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.clone());
    // Apply without waiting for assets: they stream in through the frame pump.
    let prepared = RuntimeLevel::prepare_for_project(Path::new(&level)).expect("level prepares");
    let camera = prepared.editor_camera();
    let scene: engine_backend::scene::SharedScene =
        Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    RuntimeLevel::apply_prepared(prepared, &mut scene.write().world).expect("level applies");

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe-target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut renderer = HelioRenderer::new(Arc::clone(&scene));
    {
        use engine_backend::subsystems::render::helio_renderer::pipelines;
        renderer
            .editor_mailbox()
            .set_render_pipeline(Some(pipelines::migrate_pipeline_id(
                pipelines::LEGACY_DEFAULT_PIPELINE.to_string(),
            )));
    }
    if let Some(mode) = std::env::var("PROBE_DEBUG")
        .ok()
        .and_then(|m| m.parse().ok())
    {
        renderer.editor_mailbox().set_viewport_debug_mode(mode);
    }
    if let Some(camera) = camera {
        renderer.set_editor_camera_state(EditorCameraState {
            position: camera.position,
            yaw: camera.yaw,
            pitch: camera.pitch,
        });
    }

    let seconds: u64 = std::env::var("PROBE_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    let start = Instant::now();
    let mut last_report = Instant::now() - Duration::from_secs(100);
    let mut frames = 0u64;
    let mut times: Vec<f32> = Vec::new();
    let mut settled_at: Option<Duration> = None;
    while start.elapsed() < Duration::from_secs(seconds) {
        let frame_start = Instant::now();
        renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
        renderer.render_frame(&device, &queue, &view, SIZE, SIZE, FORMAT);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        times.push(frame_start.elapsed().as_secs_f32() * 1000.0);
        if settled_at.is_none() && !scene.read().world.asset_work_pending() {
            settled_at = Some(start.elapsed());
        }
        frames += 1;
        if last_report.elapsed() > Duration::from_secs(10) {
            last_report = Instant::now();
            report(&scene, frames, start.elapsed());
        }
    }
    report(&scene, frames, start.elapsed());
    if let Ok(out) = std::env::var("PROBE_IMAGE") {
        save(&device, &queue, &texture, &out);
    }

    // Frame times while the level streamed in. The first frames build the
    // renderer's pipelines, which is not asset streaming, so skip them.
    let warm = times.len().min(30);
    let mut steady: Vec<f32> = times[warm..].to_vec();
    steady.sort_by(f32::total_cmp);
    if let (Some(worst), Some(median)) = (steady.last(), steady.get(steady.len() / 2)) {
        let p99 = steady[(steady.len() * 99 / 100).min(steady.len() - 1)];
        eprintln!(
            "PROBE frame ms: median={median:.2} p99={p99:.2} worst={worst:.2} over {} frames; assets settled at {:?}",
            steady.len(),
            settled_at.map(|t| format!("{:.1}s", t.as_secs_f32()))
        );
        if let Some(limit) = std::env::var("PROBE_FRAME_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            assert!(*worst <= limit, "worst frame {worst:.2} ms over the {limit} ms budget");
        }
    }
}

fn save(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, out: &str) {
    let row = SIZE * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * SIZE) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    image::save_buffer(out, &data, SIZE, SIZE, image::ColorType::Rgba8).unwrap();
}

fn report(scene: &engine_backend::scene::SharedScene, frames: u64, elapsed: Duration) {
    use helio_mats::instance::MaterialInstanceComponent;
    let scene = scene.read();
    let world = &scene.world;
    let mirror = world.gpu_mirror().cloned();
    let mut total = 0;
    let mut graphs = 0;
    let mut bound = 0;
    let mut custom = 0;
    let mut fallback = 0;
    let mut errors: Vec<String> = Vec::new();
    for (_, instance) in world.query::<&MaterialInstanceComponent>() {
        total += 1;
        if !instance.source().asset.ends_with(".material") {
            continue;
        }
        graphs += 1;
        if let Some(material) = instance.bound() {
            bound += 1;
            let id = instance.source().asset_id();
            match material.resolve(id, 0, mirror.as_ref(), &instance.source().fallback) {
                Ok(resolved) if resolved.material_class == helio_mats::MATERIAL_CLASS_CUSTOM => {
                    custom += 1
                }
                Ok(_) => fallback += 1,
                Err(error) => errors.push(error),
            }
        }
    }
    let store = mirror.as_ref().and_then(|m| m.texture_store());
    let slots = store
        .as_ref()
        .map(|s| s.read().unwrap().slot_count())
        .unwrap_or(0);
    let held = store
        .as_ref()
        .map(|s| {
            let s = s.read().unwrap();
            (0..s.slot_count())
                .filter(|i| s.texture(*i).is_some())
                .count()
        })
        .unwrap_or(0);
    eprintln!(
        "PROBE t={:.0}s frames={frames} instances={total} graph_instances={graphs} bound={bound} resolve_custom={custom} resolve_other={fallback} errors={} slots={slots} held={held} bias={}",
        elapsed.as_secs_f32(),
        errors.len(),
        helio_component::texture_detail::detail_bias(),
    );
    for error in errors.iter().take(5) {
        eprintln!("PROBE   error: {error}");
    }
}
