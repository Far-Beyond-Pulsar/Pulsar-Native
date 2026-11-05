//! A plugin a project does not enable is absent (Phase C step 2): with the
//! voxel, lights and materials plugins disabled in the project's
//! `Pulsar.toml`, everything installed is linked (as in the editor), yet
//!
//! - their components are not registered: a level holding them loads, keeps
//!   their data verbatim (unresolved) for saving, and refuses new ones;
//! - their Helio features are not in the renderer's feature set, so none of
//!   their passes is in a graph, and the frame's stand-ins take their place
//!   (`Unlit` for the lighting model, no material texture table input);
//! - their settings groups and tool modes are absent;
//! - a mesh still renders, unlit, with its own albedo.
//!
//! Re-enabling them (another project) brings them back. A game built for
//! such a project does not even link them: `generated_game_links_only_enabled
//! _plugins` in engine_backend's project builder tests checks its manifest.
//! One test: the plugin set is process-wide. Needs a GPU adapter for the
//! render part (skipped without one).

use std::path::Path;
use std::sync::{Arc, Mutex};

use engine_backend::scene::{attachments, level_rules, RuntimeLevel, SceneWorldExt, SharedScene};
use engine_backend::subsystems::render::helio_renderer::render_features;
use engine_backend::subsystems::render::{
    DeviceOwner, FrameProjection, FrameView, SceneRenderer, SceneRendererOptions,
};
use helio::{DebugDrawState, PassBuildContext, RendererConfig};
use helio_core::GpuCameraUniforms;
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};
use serde_json::json;

// Every installed plugin is linked, as the editor links them.
use installed_plugins as _;

const DISABLED: [&str; 3] = ["pulsar.voxel", "pulsar.lights", "pulsar.materials"];
const SIZE: u32 = 128;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn project(enabled: Option<bool>) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let mut file = String::from("# a test project\n");
    if let Some(enabled) = enabled {
        file.push_str("[plugins]\n");
        for id in DISABLED {
            file.push_str(&format!("\"{id}\" = {enabled}\n"));
        }
    }
    std::fs::write(project.path().join("Pulsar.toml"), file).unwrap();
    std::fs::create_dir_all(project.path().join("meshes")).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/meshes/primitives/SM_Cube.fbx"),
        project.path().join("meshes/cube.fbx"),
    )
    .unwrap();
    project
}

/// A level with a cube (a mesh, which stays) and a light and a voxel
/// terrain on it (which go).
fn write_level(project: &Path) -> std::path::PathBuf {
    let level = json!({
        "version": "2.1",
        "objects": [{
            "id": "cube", "name": "Cube", "object_type": { "Mesh": "Custom" },
            "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
            "parent": null, "visible": true, "locked": false, "props": {}
        }],
        "components": { "cube": [
            { "index": 0, "class_name": "StaticMeshComponent", "enabled": true, "data": {
                "mesh_asset": "meshes/cube.fbx",
                "legacy_material_override": {
                    "base_color": [1.0, 0.5, 0.1, 1.0], "metallic": 0.0, "roughness": 0.7,
                    "emissive_color": [0.0, 0.0, 0.0], "emissive_intensity": 0.0, "alpha": 1.0
                }
            } },
            { "index": 1, "class_name": "LightComponent", "enabled": true, "data": { "kept": "verbatim" } },
            { "index": 2, "class_name": "VoxelTerrainComponent", "enabled": true, "data": { "radius": 42 } }
        ] },
        "metadata": {}
    });
    let path = project.join("absent.level");
    std::fs::write(&path, level.to_string()).unwrap();
    path
}

fn registered(class: &str) -> bool {
    pulsar_world_registry::component_id_for_class(class).is_some()
}

const ABSENT_FEATURES: [&str; 7] = [
    "helio.voxel",
    "helio.lights.binning",
    "helio.lights.shadow_views",
    "helio.lights.deferred",
    "helio.hlfs",
    "helio.billboards",
    "pulsar.materials",
];

#[test]
fn a_disabled_plugin_is_absent() {
    engine_state::EngineContext::new().set_global();
    let off = project(Some(false));
    engine_state::set_project_path(off.path().display().to_string());

    // The project's plugins: everything but the three.
    for id in DISABLED {
        assert!(!pulsar_plugins::is_enabled(id), "{id} is disabled");
    }
    for id in [
        "pulsar.meshes",
        "pulsar.decals",
        "pulsar.sky",
        "pulsar.water",
    ] {
        assert!(pulsar_plugins::is_enabled(id), "{id} stays enabled");
    }

    // Their components are not registered; others are.
    for class in [
        "LightComponent",
        "ReflectionCaptureComponent",
        "VoxelTerrainComponent",
    ] {
        assert!(!registered(class), "{class} is registered");
    }
    for class in [
        "StaticMeshComponent",
        "DecalComponent",
        "AtmosphereComponent",
    ] {
        assert!(registered(class), "{class} is not registered");
    }

    // Their features are not in the renderer's set; the others' are.
    let features = render_features::plugin_features();
    for id in ABSENT_FEATURES {
        assert!(!features.contains(id), "{id} is in the feature set");
    }
    assert!(features.contains("helio.decals") || features.ids().len() > 10);

    // Their settings groups are absent.
    let groups: Vec<_> = pulsar_settings::project::rendering::groups()
        .iter()
        .map(|group| group.key)
        .collect();
    assert!(
        groups
            .iter()
            .all(|key| !key.contains("voxel") && !key.starts_with("shadows")),
        "{groups:?}"
    );

    // A level holding them loads; their instances keep their data verbatim.
    let level = write_level(off.path());
    let scene = RuntimeLevel::load(&level).expect("the level loads").scene();
    {
        let scene = scene.read();
        let cube = scene.world.entity_for("cube").unwrap();
        let records = pulsar_world_registry::component_records(&scene.world, cube);
        let classes: Vec<_> = records.iter().map(|r| r.class_name.as_str()).collect();
        assert_eq!(
            classes,
            [
                "StaticMeshComponent",
                "LightComponent",
                "VoxelTerrainComponent"
            ]
        );
        assert_eq!(records[1].data["kept"], json!("verbatim"));
        assert_eq!(records[2].data["radius"], json!(42));
        let instances = attachments::instances(&scene.world, cube);
        assert!(scene
            .world
            .get::<attachments::UnresolvedComponent>(instances[1])
            .is_some());
        // Adding one is refused.
        assert_eq!(
            level_rules::check_new_component(&scene.world, "LightComponent", None, true),
            Err(level_rules::NOT_REGISTERED)
        );
        assert_eq!(
            level_rules::check_new_component(&scene.world, "DecalComponent", None, true),
            Ok(())
        );
    }

    render_with_stand_ins(&scene);

    // Another project enables them: they are back.
    let on = project(Some(true));
    engine_state::set_project_path(on.path().display().to_string());
    for class in ["LightComponent", "VoxelTerrainComponent"] {
        assert!(registered(class), "{class} is back");
    }
    let features = render_features::plugin_features();
    for id in ABSENT_FEATURES {
        assert!(features.contains(id), "{id} is back in the feature set");
    }
    // A project that does not list them has every plugin's default (on).
    let silent = project(None);
    engine_state::set_project_path(silent.path().display().to_string());
    assert!(registered("LightComponent"));
}

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
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

/// The engine's presets without the three: no light, voxel or material
/// passes or inputs, the spine's `Unlit` resolving the G-buffer; and the
/// cube renders, unlit, in its albedo.
fn render_with_stand_ins(scene: &SharedScene) {
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: absent plugins render");
        return;
    };
    let buffer = |label: &str, size: u64| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let camera = buffer(
        "Scene Camera Buffer",
        (std::mem::size_of::<GpuCameraUniforms>() * 2) as u64,
    );
    let debug_camera = buffer("Debug Camera Buffer", 64);
    let cull_stats = buffer("Cull Stats Buffer", 64);
    let gpu_context = EngineGpuContext::new(Arc::clone(&device), Arc::clone(&queue));
    let store = SceneGpuStore::new(
        &gpu_context,
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    let scene_db = GpuMirrorHandle::new(Arc::new(store), Arc::clone(&queue));
    let features = render_features::plugin_features();
    let config = render_features::configure(RendererConfig::new(SIZE, SIZE, FORMAT));
    for preset in Preset::ALL {
        let graph = preset.with_features(&features).build(PassBuildContext {
            device: &device,
            queue: &queue,
            config: config.clone(),
            debug_state: Arc::new(Mutex::new(DebugDrawState::default())),
            camera_buffer: &camera,
            debug_camera_buffer: &debug_camera,
            cull_stats_buffer: &cull_stats,
            owns_device: false,
            scene_db: scene_db.clone(),
        });
        let passes: Vec<_> = graph
            .build_order_declarations()
            .into_iter()
            .map(|(pass, _)| pass.pass)
            .collect();
        for name in [
            "ShadowMatrix",
            "LightCull",
            "DeferredLight",
            "HLFS",
            "Billboard",
        ] {
            assert!(
                !passes.contains(&name),
                "{}: {name} is in the graph",
                preset.name()
            );
        }
        assert!(
            passes.iter().all(|pass| !pass.contains("Voxel")),
            "{}: a voxel pass is in the graph: {passes:?}",
            preset.name()
        );
        assert!(
            !graph
                .external_input_names()
                .contains(&helio_mats::MATERIAL_TEXTURES),
            "{}: the material texture table is an input",
            preset.name()
        );
        if passes.contains(&"GBuffer") {
            assert!(
                passes.contains(&"Unlit"),
                "{}: nothing resolves the G-buffer: {passes:?}",
                preset.name()
            );
        }
    }

    // The cube, unlit: its albedo (orange) where it is drawn.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("absent-plugins-target"),
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
    let mut renderer = SceneRenderer::new(
        Arc::clone(&device),
        Arc::clone(&queue),
        scene,
        SIZE,
        SIZE,
        FORMAT,
        SceneRendererOptions::game(DeviceOwner::Renderer, false),
    );
    // Framed from the cube's own size, as the runtime parity test does.
    let radius = {
        let scene = scene.read();
        let cube = scene.world.entity_for("cube").unwrap();
        let instance = attachments::instances(&scene.world, cube)[0];
        scene
            .world
            .get::<helio_component::components::StaticMeshComponent>(instance)
            .expect("the cube's mesh is registered (the meshes plugin is enabled)")
            .bounds_local[3]
    };
    assert!(radius > 0.0, "the cube's geometry loaded");
    let distance = radius * 4.0;
    let eye = glam::Vec3::new(0.0, distance / 6.0, distance);
    let camera = FrameView {
        eye: eye.as_dvec3(),
        forward: (-eye).normalize(),
        up: glam::Vec3::Y,
        aspect: 1.0,
        projection: FrameProjection::Perspective {
            fov_y: std::f32::consts::FRAC_PI_4,
            near: 0.1,
            far: distance * 10.0,
        },
        crop: None,
    };
    for _ in 0..6 {
        renderer.step_scene(&mut scene.write());
        let frame = renderer.prepare_frame(&camera, [SIZE, SIZE]);
        renderer.render(&frame.camera, &view).expect("render");
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    let pixels = read(&device, &queue, &texture);
    let centre = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    let [r, g, b] = [pixels[centre], pixels[centre + 1], pixels[centre + 2]];
    println!("ABSENT_PLUGINS centre pixel {r} {g} {b}");
    if let Ok(dir) = std::env::var("ABSENT_PLUGINS_DUMP") {
        image::save_buffer(
            Path::new(&dir).join("absent_plugins.png"),
            &pixels,
            SIZE,
            SIZE,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
    assert!(
        r > g && g > b && r > 60,
        "the cube is not drawn in its albedo: {r} {g} {b}"
    );
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let row = SIZE * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (padded * SIZE) as u64,
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
    queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
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
