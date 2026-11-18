//! Real frames of a skinned mesh and a tree on the engine path (the editor's
//! headless `HelioRenderer`, the shipped deferred graph, hardware ray
//! queries so the meshes' ray-traced scene and GI run): a bending tube
//! skinned through the meshes' skin cache, a trunk-and-canopy tree, a ground
//! and a sun. Writes the lit frame and the "Indirect Lighting" view (where
//! the traced occlusion of GI shows) at rest and bent, into
//! `SKINNED_FRAMES_DIR` (default `target-captures/r1-tlas`), and the frames'
//! raw RGBA for an A/B against a baseline build of this same file.
//!
//! Run by hand (it writes images): `skinned_frames --ignored --nocapture`.
//! Needs hardware ray queries; skips without them.

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use installed_plugins as _;
use installed_plugins::meshes_plugin::meshes_component::components::{
    MeshAssetPath, MeshGeometryKey, SkeletalMeshComponent, StaticMeshComponent,
    StaticMeshMaterialSlot, StaticMeshMaterialSlots,
};
use installed_plugins::meshes_plugin::meshes_component::mesh_cache::{
    ImportedSurfaceMaterial, MeshSection,
};
use ui_level_editor::LevelEditorState;

use glam::{Mat4, Vec3};
use installed_plugins::meshes_plugin::helio_mesh_api::{PackedVertex, SkinVertex};
use std::sync::Arc;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The deferred lighting's "Indirect Lighting" view (`helio_pass_deferred_light`).
const INDIRECT_VIEW: u32 = 23;

fn shipped_pipeline() -> String {
    use engine_backend::subsystems::render::helio_renderer::pipelines;
    if engine_state::get_project_path().is_none() {
        engine_state::EngineContext::new().set_global();
        engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    }
    pipelines::migrate_pipeline_id(pipelines::LEGACY_DEFAULT_PIPELINE.to_string())
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let features = helio::required_wgpu_features(helio::usable_adapter_features(&adapter));
    if !features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
        eprintln!("no ray queries: skipped");
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()
}

fn surface(color: [f32; 3], roughness: f32) -> ImportedSurfaceMaterial {
    ImportedSurfaceMaterial {
        base_color: [color[0], color[1], color[2], 1.0],
        roughness,
        metallic: 0.0,
        emissive: [0.0; 3],
        emissive_intensity: 0.0,
        alpha: 1.0,
    }
}

/// Axis-aligned boxes (centre, half extent) as one static mesh drawn with
/// `color`.
fn boxes(key: u128, parts: &[(Vec3, Vec3)], color: [f32; 3]) -> StaticMeshComponent {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for &(centre, half) in parts {
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let n =
                    Vec3::from_array(std::array::from_fn(|k| if k == axis { sign } else { 0.0 }));
                let u = Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 1) % 3)));
                let v = Vec3::from_array(std::array::from_fn(|k| f32::from(k == (axis + 2) % 3)));
                let (u, v) = if sign > 0.0 { (u, v) } else { (v, u) };
                let base = vertices.len() as u32;
                for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let p = centre + (n + u * a + v * b) * half;
                    vertices.push(PackedVertex::from_components(
                        p.to_array(),
                        n.to_array(),
                        [(a + 1.0) * 0.5, (b + 1.0) * 0.5],
                        u.to_array(),
                        1.0,
                    ));
                }
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
    }
    let radius = vertices
        .iter()
        .map(|v| Vec3::from(v.position).length())
        .fold(0.0f32, f32::max);
    StaticMeshComponent {
        mesh_asset: MeshAssetPath::new(""),
        geometry_key: MeshGeometryKey(key),
        material_slots: StaticMeshMaterialSlots {
            slots: vec![StaticMeshMaterialSlot {
                surface_override: Some(surface(color, 0.8)),
                ..Default::default()
            }],
        },
        mesh_sections: vec![MeshSection {
            first_index: 0,
            index_count: indices.len() as u32,
            material_slot: 0,
        }],
        vertices,
        indices,
        bounds_local: [0.0, 0.0, 0.0, radius],
        ..Default::default()
    }
}

/// A tube 3 m tall, 0.4 m across, on two joints: `root` at its foot and
/// `upper` at 1.5 m, its vertices from 1.2 m to 1.8 m blending between them.
fn tube(mesh: &mut SkeletalMeshComponent) {
    let (rings, segments) = (25u32, 24u32);
    let mut vertices = Vec::new();
    let mut skin = Vec::new();
    for r in 0..rings {
        let y = 3.0 * r as f32 / (rings - 1) as f32;
        let w = ((y - 1.2) / 0.6).clamp(0.0, 1.0);
        for s in 0..segments {
            let a = std::f32::consts::TAU * s as f32 / segments as f32;
            let n = Vec3::new(a.cos(), 0.0, a.sin());
            vertices.push(PackedVertex::from_components(
                (n * 0.4 + Vec3::Y * y).to_array(),
                n.to_array(),
                [s as f32 / segments as f32, y / 3.0],
                [-a.sin(), 0.0, a.cos()],
                1.0,
            ));
            skin.push(SkinVertex {
                joints: [0, 1, 0, 0],
                weights: [1.0 - w, w, 0.0, 0.0],
            });
        }
    }
    let mut indices = Vec::new();
    for r in 0..rings - 1 {
        for s in 0..segments {
            let (a, b) = (r * segments + s, r * segments + (s + 1) % segments);
            let (c, d) = (a + segments, b + segments);
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mut asset = skeletal_assets::skmesh::SkeletalMeshAsset::new(None);
    asset.joints = vec![
        skeletal_assets::skmesh::Joint {
            bone: "root".into(),
            inverse_bind: Mat4::IDENTITY.to_cols_array(),
        },
        skeletal_assets::skmesh::Joint {
            bone: "upper".into(),
            inverse_bind: Mat4::from_translation(-Vec3::Y * 1.5).to_cols_array(),
        },
    ];
    mesh.loaded_mesh = Some(Arc::new(asset));
    mesh.mesh_sections = vec![MeshSection {
        first_index: 0,
        index_count: indices.len() as u32,
        material_slot: 0,
    }];
    mesh.vertices = vertices;
    mesh.skin_vertices = skin;
    mesh.indices = indices;
    mesh.bounds_local = [0.0, 1.5, 0.0, 2.5];
    mesh.geometry_key = MeshGeometryKey(0x5ce1_0001);
}

/// The tube bent by `angle` (radians about z) at its upper joint.
fn bend(mesh: &mut SkeletalMeshComponent, angle: f32) {
    let bones: Arc<[String]> = vec!["root".to_string(), "upper".to_string()].into();
    mesh.show_pose(
        bones,
        vec![
            Mat4::IDENTITY.to_cols_array_2d(),
            (Mat4::from_translation(Vec3::Y * 1.5) * Mat4::from_rotation_z(angle))
                .to_cols_array_2d(),
        ],
    );
}

struct Scene {
    state: LevelEditorState,
    tube: pulsar_scenedb::Entity,
}

fn scene() -> Scene {
    let state = LevelEditorState::new();
    let tube_instance;
    {
        let mut world = state.scene.world_mut();
        let mut place = |name: &str, position: [f64; 3]| {
            let object = world
                .spawn_object(engine_backend::scene::SpawnObject::new(name))
                .expect("spawn");
            world
                .get_mut::<engine_backend::scene::Transform>(object)
                .unwrap()
                .position = position;
            object
        };
        let ground = place("ground", [0.0, 0.0, 0.0]);
        let tree = place("tree", [3.0, 0.0, -1.0]);
        let character = place("tube", [-1.5, 0.0, 0.0]);
        let sun = place("sun", [0.0, 10.0, 0.0]);
        let ground_mesh = boxes(
            0x5ce1_0010,
            &[(Vec3::new(0.0, -0.1, 0.0), Vec3::new(20.0, 0.1, 20.0))],
            [0.6, 0.6, 0.6],
        );
        pulsar_world_registry::attach_value(&mut world, ground, ground_mesh).unwrap();
        let tree_mesh = boxes(
            0x5ce1_0011,
            &[
                (Vec3::new(0.0, 1.5, 0.0), Vec3::new(0.2, 1.5, 0.2)),
                (Vec3::new(0.0, 3.5, 0.0), Vec3::new(1.4, 0.8, 1.4)),
                (Vec3::new(0.6, 4.6, 0.3), Vec3::new(0.8, 0.5, 0.8)),
            ],
            [0.25, 0.5, 0.2],
        );
        pulsar_world_registry::attach_value(&mut world, tree, tree_mesh).unwrap();
        tube_instance = pulsar_world_registry::attach_value(
            &mut world,
            character,
            SkeletalMeshComponent::default(),
        )
        .unwrap();
        {
            let mut mesh = world
                .get_mut::<SkeletalMeshComponent>(tube_instance)
                .unwrap();
            tube(&mut mesh);
            bend(&mut mesh, 0.0);
        }
        let mut sun_light =
            installed_plugins::lights_plugin::lights_component::LightComponent::default();
        sun_light.general.light_type =
            installed_plugins::lights_plugin::lights_component::LightType::Directional;
        sun_light.intensity.intensity = 6.0;
        pulsar_world_registry::attach_value(&mut world, sun, sun_light).unwrap();
        // A low sun from the right, in front (YXZ Euler degrees).
        world
            .get_mut::<engine_backend::scene::Transform>(sun)
            .unwrap()
            .rotation = [-40.0, 35.0, 0.0];
    }
    Scene {
        state,
        tube: tube_instance,
    }
}

struct Target {
    texture: wgpu::Texture,
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let row = WIDTH * 4;
    let padded = row.div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * HEIGHT),
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
                rows_per_image: Some(HEIGHT),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range().unwrap();
    data.chunks_exact(padded as usize)
        .flat_map(|line| line[..row as usize].iter().copied())
        .collect()
}

fn save(dir: &std::path::Path, name: &str, rgba: &[u8]) {
    std::fs::write(dir.join(format!("{name}.rgba")), rgba).unwrap();
    image::RgbaImage::from_raw(WIDTH, HEIGHT, rgba.to_vec())
        .unwrap()
        .save(dir.join(format!("{name}.png")))
        .unwrap();
}

#[test]
#[ignore = "writes frames; run by hand"]
fn skinned_frames() {
    let Some((device, queue)) = device() else {
        return;
    };
    let dir = std::path::PathBuf::from(
        std::env::var("SKINNED_FRAMES_DIR").unwrap_or_else(|_| "target-captures/r1-tlas".into()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let tag = std::env::var("SKINNED_FRAMES_TAG").unwrap_or_else(|_| "new".into());
    let pipeline = shipped_pipeline();
    let target = Target {
        texture: device.create_texture(&wgpu::TextureDescriptor {
            label: Some("skinned-frames"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }),
    };
    let view = target.texture.create_view(&Default::default());
    // Temporal super resolution at native scale: the final frame then
    // depends on the motion vectors (the skinned mesh's previous positions),
    // so the moving captures compare them too.
    let config = engine_state::settings::global_config();
    pulsar_settings::project::rendering::register(config);
    if let Some(rendering) = config.owner_handle(engine_state::settings::NS_PROJECT, "rendering") {
        rendering
            .set("tsr_quality", "native".to_string())
            .expect("tsr setting");
    } else {
        println!("no rendering settings group: TSR stays at the project default");
    }
    let scene = scene();
    let mut renderer = HelioRenderer::new(scene.state.scene.shared_scene());
    renderer.set_editor_camera_state(EditorCameraState {
        position: [0.0, 3.0, 9.0],
        yaw: 0.0,
        pitch: -0.25,
    });
    renderer
        .editor_mailbox()
        .set_render_pipeline(Some(pipeline));
    let render = |renderer: &mut HelioRenderer, frames: usize| {
        for _ in 0..frames {
            renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
            let _ = renderer.render_frame(&device, &queue, &view, WIDTH, HEIGHT, FORMAT);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
        read(&device, &queue, &target.texture)
    };
    let pose = |angle: f32| {
        let mut world = scene.state.scene.world_mut();
        let mut mesh = world.get_mut::<SkeletalMeshComponent>(scene.tube).unwrap();
        bend(&mut mesh, angle);
    };
    // GI probes settle over many frames: the first observation waits long.
    for (mode, mode_name) in [(0u32, "lit"), (INDIRECT_VIEW, "indirect")] {
        renderer.editor_mailbox().set_viewport_debug_mode(mode);
        pose(0.0);
        let rest = render(&mut renderer, if mode == 0 { 240 } else { 120 });
        save(&dir, &format!("{tag}_{mode_name}_rest"), &rest);
        pose(0.9);
        let bent = render(&mut renderer, 120);
        save(&dir, &format!("{tag}_{mode_name}_bent"), &bent);
        // Mid-animation: the pose moving every frame.
        for k in 0..30 {
            pose(0.9 - 0.03 * k as f32);
            render(&mut renderer, 1);
        }
        let moving = render(&mut renderer, 0);
        save(&dir, &format!("{tag}_{mode_name}_moving"), &moving);
    }
    // The same bent pose on a renderer that never saw the rest pose: the
    // reference for "did the shadow follow the pose".
    {
        let mut fresh = HelioRenderer::new(scene.state.scene.shared_scene());
        fresh.set_editor_camera_state(EditorCameraState {
            position: [0.0, 3.0, 9.0],
            yaw: 0.0,
            pitch: -0.25,
        });
        fresh
            .editor_mailbox()
            .set_render_pipeline(Some(shipped_pipeline()));
        pose(0.9);
        let bent = render(&mut fresh, 240);
        save(&dir, &format!("{tag}_lit_bent_fresh"), &bent);
    }
    // What rays see: the indirect view with the tree, then also the tube,
    // left out of the ray-traced scene by its culling (`ray_tracing`'s
    // radius, the angle at 90 degrees). The tube's centre is 9.2 m from the
    // eye, the tree's 10.9 m. A traced instance leaves only past 1.1 times
    // the radius, so the scenarios go from fewest to most traced: at 8 m
    // both leave (8.8 < 9.2), at 9.6 m the tube joins and the tree stays
    // out. Only where the project settings register (the meshes'
    // `ray_tracing` group).
    if let Some(rays) = config.owner_handle(engine_state::settings::NS_PROJECT, "ray_tracing") {
        renderer
            .editor_mailbox()
            .set_viewport_debug_mode(INDIRECT_VIEW);
        pose(0.0);
        for (radius, name) in [
            (8.0f64, "traced_none"),
            (9.6, "traced_no_tree"),
            (1000.0, "traced_all"),
        ] {
            rays.set("culling_radius", radius).unwrap();
            rays.set("culling_angle", 90.0f64).unwrap();
            let frame = render(&mut renderer, 240);
            save(&dir, &format!("{tag}_indirect_{name}"), &frame);
            if let Some(stats) = renderer.frame_stats() {
                let stat = |key: &str| stats.get(key).unwrap_or(0);
                println!(
                    "{name}: {} ray instances, {} culled, skin cache {} bytes",
                    stat("meshes.ray_instances"),
                    stat("meshes.ray_instances_culled"),
                    stat("meshes.skin_cache_bytes"),
                );
            }
        }
    } else {
        println!("no ray_tracing settings group: the TLAS membership scenarios are skipped");
    }
    println!("frames in {}", dir.display());
}
