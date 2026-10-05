//! SceneDB corrective plan, Phase 0 failure baseline (Pulsar-Native#1035).
//!
//! Drives the editor's real producers (`execute_command(AddObject)` followed
//! by `scene_edit::components::add_component`, as viewport asset drop and the
//! properties panel's "Add component" do) against a headless
//! `HelioRenderer`, then records each stage a mesh or light passes on its
//! way to the screen:
//!
//! 1. typed value in `World`;
//! 2. GPU mirror of the mesh's vertex/index pools;
//! 3. the draw row the object-batch pass consumes (`StaticObjectComponent`,
//!    currently written by the CPU projection in `helio_bridge`) and the
//!    material row next to it;
//! 4. scene depth texels and color pixels that differ from an empty scene
//!    seen from the same camera. Depth separates "rasterized but shaded
//!    black" from "never drawn".
//!
//! The properties panel's influence is reproduced by calling the same
//! functions it calls: `subscribe_component` when a card is shown and
//! `take_world_component_events` once per UI frame.
//!
//! Every case runs twice: with the editor camera at rest, and with it nudged
//! on alternate frames. A camera change rebuilds Helio's Hi-Z pyramid; at
//! the pinned Helio a camera at rest keeps culling meshes that are present
//! before the first frame (regression from Helio `0e01e9fd`).
//!
//! This test only reads state. It installs no subscription, refresh or resync
//! to make anything visible, so it prints the current behavior as the baseline
//! that Phase 2 turns into assertions. It asserts only what the probes need
//! to be meaningful: the meshes hydrate, the reference frame shows the editor
//! grid, and with the camera moving a mesh that has its draw row changes
//! depth and color (positive control). Run with
//! `cargo test -p ui_level_editor --test phase0_render_baseline -- --nocapture`;
//! set `PHASE0_DUMP_DIR` to also write every observed frame as a PNG.

use std::collections::HashMap;

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use helio_component::components::{ObjectMovability, StaticMeshComponent};
use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use pulsar_scenedb::component::type_name;
use pulsar_scenedb::{Entity, World};
use serde_json::{json, Value};
use ui_level_editor::commands::{execute_command, SceneCommand};
use ui_level_editor::scene_edit::components;
use ui_level_editor::scene_edit::{LightType, MeshType, ObjectType, Transform};
use ui_level_editor::{LevelEditorState, SceneObjectData};

const SIZE: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Frames rendered per observation, so uploads, the object batch's async
/// draw-count readback and temporal filters settle. `PHASE0_SETTLE_FRAMES`
/// overrides it.
fn settle_frames() -> usize {
    std::env::var("PHASE0_SETTLE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(8)
}
/// A pixel counts as changed when its RGB channels differ by this much in sum.
const PIXEL_THRESHOLD: u32 = 48;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    // Lavapipe loses the device compiling ray-query pipelines; nothing here needs them.
    let features = adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()
}

fn mesh_asset() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/meshes/primitives/SM_Cube.fbx")
        .canonicalize()
        .expect("SM_Cube.fbx is checked in");
    // Windows canonical paths carry a `\\?\` prefix the asset resolver rejects.
    let path = path.to_string_lossy();
    path.strip_prefix(r"\\?\").unwrap_or(&path).replace('\\', "/")
}

/// One observed frame: final color and Helio's scene depth.
struct Frame {
    color: Vec<u8>,
    depth: Vec<f32>,
}

/// How a frame differs from the empty-scene reference.
#[derive(Debug, Clone, Copy)]
struct Difference {
    color_pixels: usize,
    depth_texels: usize,
}

impl Frame {
    fn difference(&self, reference: &Frame) -> Difference {
        let color_pixels = reference
            .color
            .chunks_exact(4)
            .zip(self.color.chunks_exact(4))
            .filter(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > PIXEL_THRESHOLD)
            .count();
        let depth_texels = reference
            .depth
            .iter()
            .zip(&self.depth)
            .filter(|(a, b)| (*a - *b).abs() > 1e-6)
            .count();
        Difference {
            color_pixels,
            depth_texels,
        }
    }

    /// With `PHASE0_DUMP_DIR` set, writes the color image as `<name>.png`.
    fn dump(&self, name: &str) {
        let Some(dir) = std::env::var_os("PHASE0_DUMP_DIR") else {
            return;
        };
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        image::save_buffer(&path, &self.color, SIZE, SIZE, image::ColorType::Rgba8)
            .expect("write dump");
    }
}

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    camera: EditorCameraState,
    /// Nudge the camera on alternate frames (ending at the base pose). A
    /// camera change rebuilds Helio's Hi-Z pyramid; at Helio `05c2f7d7` a
    /// camera at rest keeps a pyramid built before anything drew, which
    /// culls every object (regression from Helio `0e01e9fd`).
    nudge_camera: std::cell::Cell<bool>,
}

impl Harness {
    fn new(device: wgpu::Device, queue: wgpu::Queue, mesh_radius: f32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("phase0-baseline-target"),
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
        // Look at the origin from +Z, far enough to frame the mesh's bounds.
        let distance = (mesh_radius * 4.0).max(3.0) as f64;
        let height = distance / 6.0;
        let camera = EditorCameraState {
            position: [0.0, height, distance],
            yaw: 0.0,
            pitch: (-(height / distance)).atan() as f32,
        };
        Self {
            device,
            queue,
            texture,
            camera,
            nudge_camera: std::cell::Cell::new(false),
        }
    }

    fn renderer(&self, state: &LevelEditorState) -> HelioRenderer {
        let mut renderer = HelioRenderer::new(state.scene.shared_scene());
        renderer.set_editor_camera_state(self.camera);
        renderer
    }

    /// Renders `settle_frames()` frames, running `ui_frame` before each one as
    /// the UI thread would, and reads back the last one.
    fn frames(&self, renderer: &mut HelioRenderer, mut ui_frame: impl FnMut()) -> Frame {
        let view = self.texture.create_view(&Default::default());
        let mut encoded = false;
        let frames = settle_frames().max(2) & !1;
        for frame in 0..frames {
            ui_frame();
            if self.nudge_camera.get() {
                let yaw = self.camera.yaw + if frame % 2 == 0 { 1e-3 } else { 0.0 };
                renderer.set_editor_camera_state(EditorCameraState { yaw, ..self.camera });
            }
            // An unchanged scene makes the renderer skip the frame entirely.
            // A pending gizmo-mode change forces it to encode one without
            // syncing the scene or resetting temporal history, so the probe
            // sees the current state, never a stale first frame.
            renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
            encoded |= renderer
                .render_frame(&self.device, &self.queue, &view, SIZE, SIZE, FORMAT)
                .is_some();
            self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
        assert!(encoded, "no frame was encoded");
        let depth = renderer.debug_depth_texture().expect("renderer initialized");
        Frame {
            color: self.read_texture(&self.texture, wgpu::TextureAspect::All),
            depth: bytemuck_f32(&self.read_texture(depth, wgpu::TextureAspect::DepthOnly)),
        }
    }

    /// Tightly packed texels of a 4-byte-per-texel texture.
    fn read_texture(&self, texture: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Vec<u8> {
        let size = texture.size();
        let row = size.width * 4;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("phase0-baseline-readback"),
            size: (padded * size.height) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                aspect,
                ..texture.as_image_copy()
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size.height),
                },
            },
            wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mapped = slice.get_mapped_range().expect("mapped range");
        let data = mapped
            .chunks_exact(padded as usize)
            .flat_map(|line| line[..row as usize].iter().copied())
            .collect();
        drop(mapped);
        buffer.unmap();
        data
    }
}

fn bytemuck_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn add_object(state: &mut LevelEditorState, name: &str, object_type: ObjectType) -> String {
    let result = execute_command(
        state,
        SceneCommand::AddObject {
            data: SceneObjectData {
                id: String::new(),
                name: name.to_string(),
                object_type,
                transform: Transform::default(),
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                props: HashMap::new(),
                scene_path: String::new(),
                component_instances: None,
            },
            parent_id: None,
        },
    );
    result.affected_ids.first().cloned().expect("AddObject creates an object")
}

/// Viewport asset drop: `AddObject`, then `add_component` with the asset
/// path. A `MaterialOverrideComponent` with a bright emissive color follows,
/// so the mesh shows against the empty scene's black background without
/// depending on the light path, which is itself under test.
fn drop_mesh(state: &mut LevelEditorState) -> String {
    let id = add_object(state, "SM_Cube", ObjectType::Mesh(MeshType::Custom));
    let mut world = state.scene.world_mut();
    components::add_component(
        &mut world,
        &id,
        "StaticMeshComponent".to_string(),
        json!({ "mesh_asset": mesh_asset() }),
    );
    let mut material = class_json("MaterialOverrideComponent");
    material["base_color"] = json!([1.0, 0.5, 0.1, 1.0]);
    material["emissive_color"] = json!([1.0, 0.5, 0.1]);
    material["emissive_intensity"] = json!(4.0);
    // A default override has `alpha: 0.0`, which makes it a fully transparent
    // material (see the ledger's MaterialOverrideComponent row).
    material["alpha"] = json!(1.0);
    components::add_component(&mut world, &id, "MaterialOverrideComponent".to_string(), material);
    id
}

/// The properties panel's "Add component" payload: one JSON entry per
/// reflected property of a default instance.
fn panel_defaults(class_name: &str) -> Value {
    let instance = REGISTRY.create_instance(class_name).expect("registered class");
    let mut map = serde_json::Map::new();
    for prop in instance.get_properties() {
        let value = (prop.getter)(instance.as_ref());
        let json_value = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(value.as_ref())
            .unwrap_or(Value::Null);
        map.insert(prop.name.to_string(), json_value);
    }
    Value::Object(map)
}

fn class_json(class_name: &str) -> Value {
    REGISTRY
        .create_instance(class_name)
        .and_then(|instance| instance.to_json().ok())
        .expect("class serializes")
}

/// The shape of the light that failed to hydrate in the 2026-10-04 editor
/// log: `intensity` stored as a bare number instead of `IntensityLightProps`.
fn legacy_flat_light() -> Value {
    let mut light = class_json("LightComponent");
    light["intensity"] = json!(1002.0);
    light
}

fn set_mesh_movability(state: &LevelEditorState, id: &str, movability: ObjectMovability) {
    let result = components::update_live_component_property(
        &mut state.scene.world_mut(),
        id,
        "StaticMeshComponent",
        // `drop_mesh` attaches the mesh first, so it is instance 0.
        0,
        "movability",
        Box::new(movability),
    );
    assert!(result.is_ok(), "movability edit was refused");
}

fn entity(state: &LevelEditorState, id: &str) -> Entity {
    state.scene.world().entity_for(id).expect("object has an entity")
}

/// Short type names of every component on `entity`.
fn component_names(world: &World, entity: Entity) -> Vec<String> {
    let mut names: Vec<String> = world
        .component_ids(entity)
        .map(|id| {
            let path = type_name(id);
            let base = path.split('<').next().unwrap_or(path);
            base.rsplit("::").next().unwrap_or(base).to_string()
        })
        .collect();
    names.sort();
    names
}

#[derive(Debug)]
#[allow(dead_code)] // read through `Debug` in the printed baseline
struct MeshStages {
    typed: bool,
    gpu_vertices: u32,
    gpu_indices: u32,
    draw_row: bool,
    material_row: bool,
    on_screen: Difference,
    components: Vec<String>,
}

fn mesh_stages(state: &LevelEditorState, entity: Entity, on_screen: Difference) -> MeshStages {
    let world = state.scene.world();
    let (gpu_vertices, gpu_indices) = world
        .gpu_mirror()
        .map(|mirror| {
            let row = entity.index();
            (
                StaticMeshComponent::vertices_gpu_handle(mirror.store(), row).map_or(0, |h| h.count),
                StaticMeshComponent::indices_gpu_handle(mirror.store(), row).map_or(0, |h| h.count),
            )
        })
        .unwrap_or((0, 0));
    let paths: Vec<&str> = world.component_ids(entity).map(type_name).collect();
    MeshStages {
        typed: world.get::<StaticMeshComponent>(entity).is_some(),
        gpu_vertices,
        gpu_indices,
        draw_row: paths.iter().any(|p| p.ends_with("::StaticObjectComponent")),
        material_row: paths.iter().any(|p| p.ends_with("::MaterialComponent")),
        on_screen,
        components: component_names(&world, entity),
    }
}

fn mesh_radius() -> f32 {
    let mut world = World::new();
    let entity = world.spawn();
    pulsar_world_registry::hydrate_world_component_for_class(
        "StaticMeshComponent",
        &mut world,
        entity,
        &json!({ "mesh_asset": mesh_asset() }),
    )
    .expect("SM_Cube hydrates");
    let mesh = world.get::<StaticMeshComponent>(entity).unwrap();
    assert!(!mesh.indices.is_empty(), "SM_Cube.fbx loaded no geometry");
    mesh.bounds_local[3]
}

#[test]
fn phase0_mesh_and_light_baseline() {
    // Renderer errors and asset failures are reported only through `tracing`.
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    // Mesh hydrate requires a project root (absolute asset paths then resolve
    // as-is), and the project path lives on the global engine context.
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let harness = Harness::new(device, queue, mesh_radius());

    for nudge in [false, true] {
        harness.nudge_camera.set(nudge);
        let mode = if nudge { "camera nudging" } else { "camera at rest" };
        let reference = {
            let state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {})
        };
        reference.dump(&format!("{mode}_reference"));
        let first = &reference.color[..4];
        assert!(
            reference.color.chunks_exact(4).any(|px| px != first),
            "the empty-scene frame is uniform: color readback is not working"
        );
        let observe = |name: &str, frame: Frame| {
            frame.dump(&format!("{mode}_{name}"));
            frame.difference(&reference)
        };

        // ── Mesh present before the renderer's first frame ──────────────────────
        {
            let mut state = LevelEditorState::new();
            let id = drop_mesh(&mut state);
            let mut renderer = harness.renderer(&state);
            let frame = harness.frames(&mut renderer, || {});
            let stages = mesh_stages(&state, entity(&state, &id), observe("before_first_frame", frame));
            println!("PHASE0 [{mode}] mesh added before the first frame: {stages:#?}");
            assert!(stages.typed && stages.gpu_indices > 0, "mesh did not hydrate: {stages:#?}");
            // Positive control: with Hi-Z rebuilt by a moving camera, a mesh
            // that has its draw row reaches the image, so a zero elsewhere
            // means "not drawn", not "probe blind".
            if nudge {
                assert!(
                    stages.draw_row && stages.on_screen.color_pixels > 0 && stages.on_screen.depth_texels > 0,
                    "a projected mesh did not reach the image with the camera moving: {stages:#?}"
                );
            }
        }

        // ── Mesh dropped after the first frame, panel closed, then edited ───────
        {
            let mut state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            let id = drop_mesh(&mut state);
            let e = entity(&state, &id);
            let frame = harness.frames(&mut renderer, || {});
            println!(
                "PHASE0 [{mode}] mesh added after the first frame, panel closed: {:#?}",
                mesh_stages(&state, e, observe("after_first_frame_closed", frame))
            );
            set_mesh_movability(&state, &id, ObjectMovability::Movable);
            let frame = harness.frames(&mut renderer, || {});
            println!(
                "PHASE0 [{mode}]   ...after a movability edit: {:#?}",
                mesh_stages(&state, e, observe("after_first_frame_closed_edit", frame))
            );
        }

        // ── Panel open: the card subscribes; who drains the shared queue first ──
        for panel_drains_first in [true, false] {
            let mut state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            let id = drop_mesh(&mut state);
            let e = entity(&state, &id);
            let _card =
                components::subscribe_component(&mut state.scene.world_mut(), &id, "StaticMeshComponent");
            let (label, tag) = if panel_drains_first {
                ("panel open, panel drains first", "panel_drains")
            } else {
                ("panel open, renderer drains", "renderer_drains")
            };
            let ui_frame = || {
                if panel_drains_first {
                    components::take_world_component_events(&mut state.scene.world_mut());
                }
            };
            let frame = harness.frames(&mut renderer, ui_frame);
            println!(
                "PHASE0 [{mode}] mesh added after the first frame, {label}: {:#?}",
                mesh_stages(&state, e, observe(tag, frame))
            );
            set_mesh_movability(&state, &id, ObjectMovability::Movable);
            let frame = harness.frames(&mut renderer, ui_frame);
            println!(
                "PHASE0 [{mode}]   ...after a movability edit: {:#?}",
                mesh_stages(&state, e, observe(&format!("{tag}_edit"), frame))
            );
        }

        // ── Light added after the first frame, next to a mesh ───────────────────
        for (label, data) in [
            ("panel payload", panel_defaults("LightComponent")),
            ("class to_json", class_json("LightComponent")),
            ("legacy flat intensity", legacy_flat_light()),
        ] {
            let mut state = LevelEditorState::new();
            drop_mesh(&mut state);
            let mut renderer = harness.renderer(&state);
            let unlit = harness.frames(&mut renderer, || {});
            let id = add_object(&mut state, "Light", ObjectType::Light(LightType::Point));
            // Above and in front of the mesh, between it and the camera.
            let [_, y, z] = harness.camera.position;
            execute_command(
                &mut state,
                SceneCommand::SetTransform {
                    id: id.clone(),
                    position: Some([0.0, y as f32 * 2.0, z as f32 * 0.5]),
                    rotation: None,
                    scale: None,
                },
            );
            components::add_component(&mut state.scene.world_mut(), &id, "LightComponent".to_string(), data);
            let lit = harness.frames(&mut renderer, || {});
            let tag = format!("light_{}", label.replace(' ', "_"));
            lit.dump(&format!("{mode}_{tag}"));
            let world = state.scene.world();
            let e = world.entity_for(&id).unwrap();
            let attachments: Vec<(String, bool, bool)> = components::get_components_metadata(&world, &id)
                .into_iter()
                .map(|c| (c.class_name, c.enabled, !c.data.is_null() && c.data != json!({})))
                .collect();
            println!(
                "PHASE0 [{mode}] light added after the first frame ({label}): typed = {}, attachments (class, enabled, keeps JSON) = {attachments:?}, components = {:?}, vs unlit = {:?}",
                world.get::<helio_component::components::LightComponent>(e).is_some(),
                component_names(&world, e),
                lit.difference(&unlit),
            );
        }
    }
}
