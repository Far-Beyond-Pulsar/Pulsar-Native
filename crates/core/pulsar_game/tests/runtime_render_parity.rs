//! Runtime parity (Pulsar-Native#1035 acceptance, #1081): one level file,
//! loaded by the runtime loader, renders its mesh through the standalone
//! game's renderer, the embedded (Play-in-Editor) game's renderer and the
//! editor viewport's, all headless; and a disabled mesh is drawn by none.
//!
//! The standalone and embedded renderers are the ones the game window and
//! the PIE viewport build (`pulsar_game::game_renderer`); only the window
//! surface is replaced by an offscreen texture. Needs a GPU adapter
//! (lavapipe works); skips without one.

use std::path::Path;
use std::sync::Arc;

use engine_backend::scene::{attachments, RuntimeLevel, SceneWorldExt, SharedScene};
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use helio_component::components::StaticMeshComponent;
use pulsar_game::game_renderer::{build_game_renderer, project_renderer_config, DeviceOwner};
use serde_json::json;

const SIZE: u32 = 192;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const FRAMES: usize = 6;

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    // Lavapipe loses the device compiling ray-query pipelines; nothing here needs them.
    let features = adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

struct Target {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    texture: wgpu::Texture,
}

impl Target {
    fn read(&self) -> Vec<u8> {
        let row = SIZE * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded * SIZE) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(SIZE),
                },
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let data = buffer
            .slice(..)
            .get_mapped_range()
            .unwrap()
            .chunks_exact(padded as usize)
            .flat_map(|line| line[..row as usize].to_vec())
            .collect();
        buffer.unmap();
        data
    }
}

/// Pixels whose RGB differ by more than a small threshold.
fn changed(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > 48)
        .count()
}

#[derive(Clone, Copy, Debug)]
enum Runtime {
    Standalone,
    Embedded,
    Editor,
}

/// Render `scene` through `runtime` for a few frames from a camera looking
/// at the origin from `distance`, and read the image back.
fn render(runtime: Runtime, target: &Target, scene: &SharedScene, distance: f32) -> Vec<u8> {
    let view = target.texture.create_view(&Default::default());
    let eye = glam::Vec3::new(0.0, distance / 6.0, distance);
    match runtime {
        Runtime::Editor => {
            let mut renderer = HelioRenderer::new(Arc::clone(scene));
            renderer.set_editor_camera_state(EditorCameraState {
                position: [eye.x as f64, eye.y as f64, eye.z as f64],
                yaw: 0.0,
                pitch: (-(eye.y / eye.z)).atan(),
            });
            for _ in 0..FRAMES {
                renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
                renderer.render_frame(&target.device, &target.queue, &view, SIZE, SIZE, FORMAT);
                target
                    .device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .unwrap();
            }
        }
        Runtime::Standalone | Runtime::Embedded => {
            let (config, owner) = match runtime {
                Runtime::Standalone => (
                    project_renderer_config(SIZE, SIZE, FORMAT),
                    DeviceOwner::Game,
                ),
                _ => (
                    helio::RendererConfig::new(SIZE, SIZE, FORMAT),
                    DeviceOwner::Host,
                ),
            };
            let mut renderer = build_game_renderer(
                Arc::clone(&target.device),
                Arc::clone(&target.queue),
                scene,
                config,
                false,
                owner,
            );
            let camera = helio::Camera::perspective_look_at(
                eye,
                glam::Vec3::ZERO,
                glam::Vec3::Y,
                std::f32::consts::FRAC_PI_4,
                1.0,
                0.1,
                distance * 10.0,
            );
            for _ in 0..FRAMES {
                // As the game loop and the PIE tick do: step the world, then render.
                scene.write().step();
                renderer.render(&camera, &view).expect("render");
                target
                    .device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .unwrap();
            }
        }
    }
    target.read()
}

#[test]
fn every_runtime_renders_the_same_level() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("meshes")).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/meshes/primitives/SM_Cube.fbx"),
        project.path().join("meshes/cube.fbx"),
    )
    .unwrap();
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());

    let object = |components: serde_json::Value| {
        json!({
            "version": "2.1",
            "objects": [{
                "id": "cube", "name": "Cube", "object_type": { "Mesh": "Custom" },
                "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
                "parent": null, "visible": true, "locked": false, "props": {}
            }],
            "components": { "cube": components },
            "metadata": {}
        })
    };
    // An emissive surface, so the mesh shows without depending on lights.
    let mesh = json!({
        "mesh_asset": "meshes/cube.fbx",
        "legacy_material_override": {
            "base_color": [1.0, 0.5, 0.1, 1.0], "metallic": 0.0, "roughness": 0.7,
            "emissive_color": [1.0, 0.5, 0.1], "emissive_intensity": 4.0, "alpha": 1.0
        }
    });
    let write = |name: &str, level: serde_json::Value| {
        let path = project.path().join(name);
        std::fs::write(&path, level.to_string()).unwrap();
        path
    };
    let with_mesh = write(
        "mesh.level",
        object(
            json!([{ "index": 0, "class_name": "StaticMeshComponent", "data": mesh, "enabled": true }]),
        ),
    );
    let empty = write("empty.level", object(json!([])));
    let load = |path: &Path| RuntimeLevel::load(path).expect("level loads").scene();

    let radius = {
        let scene = load(&with_mesh);
        let scene = scene.read();
        let cube = scene.world.entity_for("cube").unwrap();
        let instance = attachments::instances(&scene.world, cube)[0];
        scene
            .world
            .get::<StaticMeshComponent>(instance)
            .unwrap()
            .bounds_local[3]
    };
    assert!(radius > 0.5, "the cube's geometry loaded");
    let distance = radius * 4.0;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("runtime-parity-target"),
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
    let target = Target {
        device,
        queue,
        texture,
    };

    for runtime in [Runtime::Standalone, Runtime::Embedded, Runtime::Editor] {
        let reference = render(runtime, &target, &load(&empty), distance);
        let scene = load(&with_mesh);
        let drawn = changed(&render(runtime, &target, &scene, distance), &reference);
        println!("PHASE7 {runtime:?}: mesh changes {drawn} pixels");
        assert!(
            drawn > (SIZE * SIZE / 50) as usize,
            "{runtime:?} did not draw the mesh"
        );

        {
            let mut scene = scene.write();
            let cube = scene.world.entity_for("cube").unwrap();
            let instance = attachments::instances(&scene.world, cube)[0];
            attachments::set_enabled(&mut scene.world, instance, false);
        }
        let hidden = changed(&render(runtime, &target, &scene, distance), &reference);
        assert!(
            hidden < (SIZE * SIZE / 200) as usize,
            "{runtime:?} still draws a disabled mesh ({hidden} pixels)"
        );
    }
}
