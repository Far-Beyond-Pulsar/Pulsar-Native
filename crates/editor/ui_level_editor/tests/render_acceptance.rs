//! SceneDB corrective plan, Phase 2 acceptance (Pulsar-Native#1035): meshes
//! and lights reach the rendered frame through the actual pass path, from
//! every producer, with no subscription, refresh, resync or inspector.
//!
//! Drives the editor's real producers (`execute_command`, the
//! `scene_edit::components` functions the properties panel and viewport
//! asset drop call) and direct typed inserts against a headless
//! `HelioRenderer`, and compares scene depth and final color with an empty
//! scene seen from the same camera. Depth separates "drawn" from "shaded
//! black"; color proves the material path. Every case runs with the camera
//! at rest and with it nudging (which rebuilds Hi-Z).
//!
//! This replaces Phase 0's read-only baseline (`11-phase-0-closure.md`
//! records its table): the cases it printed are assertions here. Run with
//! `cargo test -p ui_level_editor --test render_acceptance -- --nocapture`;
//! set `RENDER_ACCEPTANCE_DUMP_DIR` to also write every observed frame as a
//! PNG.

use std::collections::HashMap;

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use helio_component::components::{
    MeshAssetPath, ObjectMovability, StaticMeshComponent, StaticMeshMaterialSlot,
    StaticMeshMaterialSlots,
};
use pulsar_reflection::REGISTRY;
use pulsar_scenedb::Entity;
use serde_json::{json, Value};
use ui_level_editor::commands::{execute_command, SceneCommand};
use ui_level_editor::scene_edit::components;
use ui_level_editor::scene_edit::{LightType, MeshType, ObjectType, Transform};
use ui_level_editor::{LevelEditorState, SceneObjectData};

const SIZE: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Frames rendered per observation, so uploads, the object batch's async
/// draw-count readback and temporal filters settle. `RENDER_ACCEPTANCE_SETTLE_FRAMES`
/// overrides it.
fn settle_frames() -> usize {
    std::env::var("RENDER_ACCEPTANCE_SETTLE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
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
    path.strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .replace('\\', "/")
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
            .filter(|(a, b)| {
                (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > PIXEL_THRESHOLD
            })
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

    /// With `RENDER_ACCEPTANCE_DUMP_DIR` set, writes the color image as `<name>.png`.
    fn dump(&self, name: &str) {
        let Some(dir) = std::env::var_os("RENDER_ACCEPTANCE_DUMP_DIR") else {
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
    /// camera change rebuilds Helio's Hi-Z pyramid; Phase 0 found a camera
    /// at rest culling everything (Helio `0e01e9fd`, fixed by Helio#317),
    /// so every case runs both ways.
    nudge_camera: std::cell::Cell<bool>,
}

impl Harness {
    fn new(device: wgpu::Device, queue: wgpu::Queue, mesh_radius: f32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render-acceptance-target"),
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
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
        }
        assert!(encoded, "no frame was encoded");
        let depth = renderer
            .debug_depth_texture()
            .expect("renderer initialized");
        Frame {
            color: self.read_texture(&self.texture, wgpu::TextureAspect::All),
            depth: bytemuck_f32(&self.read_texture(depth, wgpu::TextureAspect::DepthOnly)),
        }
    }

    /// Tightly packed texels of a 4-byte-per-texel texture.
    fn read_texture(&self, texture: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Vec<u8> {
        let size = texture.size();
        let row = size.width * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("render-acceptance-readback"),
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
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
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
    result
        .affected_ids
        .first()
        .cloned()
        .expect("AddObject creates an object")
}

/// Viewport asset drop: `AddObject`, then `add_component` with the asset
/// path. Every material slot of the mesh gets a bright, opaque, emissive
/// surface so the mesh shows against the empty scene's black background
/// without depending on the light path, which is itself under test. The
/// standalone `MaterialOverrideComponent` is gone (Helio `f9631fd`); its
/// load-time migration field is the one producer that sets every slot's
/// surface without knowing the asset's slot names.
fn drop_mesh(state: &mut LevelEditorState) -> String {
    drop_mesh_with(
        state,
        json!({
            "base_color": [1.0, 0.5, 0.1, 1.0],
            "metallic": 0.0,
            "roughness": 0.7,
            "emissive_color": [1.0, 0.5, 0.1],
            "emissive_intensity": 4.0,
            "alpha": 1.0,
        }),
    )
}

/// [`drop_mesh`] with a matte white, non-emissive surface: black without a
/// light, so a light's contribution shows.
fn drop_matte_mesh(state: &mut LevelEditorState) -> String {
    drop_mesh_with(
        state,
        json!({
            "base_color": [0.8, 0.8, 0.8, 1.0],
            "metallic": 0.0,
            "roughness": 0.8,
            "emissive_color": [0.0, 0.0, 0.0],
            "emissive_intensity": 0.0,
            "alpha": 1.0,
        }),
    )
}

fn drop_mesh_with(state: &mut LevelEditorState, surface: Value) -> String {
    let id = add_object(state, "SM_Cube", ObjectType::Mesh(MeshType::Custom));
    let mut world = state.scene.world_mut();
    components::add_component(
        &mut world,
        &id,
        "StaticMeshComponent".to_string(),
        json!({
            "mesh_asset": mesh_asset(),
            "legacy_material_override": surface,
        }),
    );
    id
}

fn class_json(class_name: &str) -> Value {
    REGISTRY
        .create_instance(class_name)
        .and_then(|instance| instance.to_json().ok())
        .expect("class serializes")
}

/// `light` (a light payload) bright enough to show at the test scene's
/// scale: the cube is ~86 units across and the light ~2.5 radii away, where
/// the default 1000 lm adds a fraction of a lux.
fn bright(mut light: Value) -> Value {
    *light
        .pointer_mut("/intensity/intensity")
        .expect("the class shape") = json!(BRIGHT);
    light
}

const BRIGHT: f32 = 5.0e7;

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

/// The bright, opaque, emissive surface every acceptance mesh draws with, so
/// it shows against the empty scene without depending on lights.
fn emissive_surface() -> helio_component::mesh_cache::ImportedSurfaceMaterial {
    helio_component::mesh_cache::ImportedSurfaceMaterial {
        base_color: [1.0, 0.5, 0.1, 1.0],
        roughness: 0.7,
        metallic: 0.0,
        emissive: [1.0, 0.5, 0.1],
        emissive_intensity: 4.0,
        alpha: 1.0,
    }
}

/// A mesh built in code and inserted as a typed value: the cube's geometry,
/// sections and bounds, every slot overridden with [`emissive_surface`].
fn typed_mesh() -> StaticMeshComponent {
    let upload =
        helio_component::subsystems::load_mesh_asset_upload(std::path::Path::new(&mesh_asset()))
            .expect("SM_Cube.fbx loads");
    let radius = upload
        .geometry
        .vertices
        .iter()
        .map(|v| glam::Vec3::from(v.position).length())
        .fold(0.0f32, f32::max);
    let slots = upload
        .material_slots
        .iter()
        .map(|slot| StaticMeshMaterialSlot {
            source_material: slot.source_material,
            name: slot.name.clone(),
            imported_surface: slot.surface,
            surface_override: Some(emissive_surface()),
            ..Default::default()
        })
        .collect();
    StaticMeshComponent {
        mesh_asset: MeshAssetPath::new(mesh_asset()),
        material_slots: StaticMeshMaterialSlots { slots },
        vertices: upload.geometry.vertices,
        indices: upload.geometry.indices,
        mesh_sections: upload.sections,
        bounds_local: [0.0, 0.0, 0.0, radius],
        ..Default::default()
    }
}

/// The object's first component instance (`drop_mesh` attaches the mesh
/// first).
fn first_instance(state: &LevelEditorState, id: &str) -> Entity {
    components::instance_at(&state.scene.world(), id, 0).expect("object has a component")
}

fn set_visible(state: &mut LevelEditorState, id: &str, visible: bool) {
    let result = execute_command(
        state,
        SceneCommand::SetVisibility {
            id: id.to_string(),
            visible: Some(visible),
            locked: None,
        },
    );
    assert!(result.changed, "visibility edit was refused");
}

fn move_to(state: &mut LevelEditorState, id: &str, position: [f32; 3]) {
    let result = execute_command(
        state,
        SceneCommand::SetTransform {
            id: id.to_string(),
            position: Some(position),
            rotation: None,
            scale: None,
        },
    );
    assert!(result.changed, "transform edit was refused");
}

/// Drawn: the scene depth and the final color both differ from the empty
/// scene.
#[track_caller]
fn assert_drawn(what: &str, d: Difference) {
    println!("PHASE2 {what}: {d:?}");
    assert!(
        d.depth_texels > 0 && d.color_pixels > 0,
        "{what}: not drawn ({d:?})"
    );
}

/// Not drawn: the scene depth equals the empty scene's, and at most a
/// residue of temporally filtered color remains.
#[track_caller]
fn assert_not_drawn(what: &str, d: Difference) {
    println!("PHASE2 {what}: {d:?}");
    assert!(
        d.depth_texels == 0 && d.color_pixels < (SIZE * SIZE / 100) as usize,
        "{what}: still drawn ({d:?})"
    );
}

#[test]
fn meshes_and_lights_reach_the_frame_from_every_producer() {
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
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);

    for nudge in [false, true] {
        harness.nudge_camera.set(nudge);
        let mode = if nudge {
            "camera nudging"
        } else {
            "camera at rest"
        };
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

        // ── Editor insertion before the renderer's first frame ─────────────
        {
            let mut state = LevelEditorState::new();
            drop_mesh(&mut state);
            let mut renderer = harness.renderer(&state);
            let frame = harness.frames(&mut renderer, || {});
            assert_drawn(
                &format!("[{mode}] editor mesh, before the first frame"),
                observe("before_first_frame", frame),
            );
        }

        // ── Editor insertion after the first frame, then edits ─────────────
        {
            let mut state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            let id = drop_mesh(&mut state);
            let placed = harness.frames(&mut renderer, || {});
            let at_origin = Frame {
                color: placed.color.clone(),
                depth: placed.depth.clone(),
            };
            assert_drawn(
                &format!("[{mode}] editor mesh, after the first frame"),
                observe("after_first_frame", placed),
            );

            set_mesh_movability(&state, &id, ObjectMovability::Movable);
            let frame = harness.frames(&mut renderer, || {});
            assert_drawn(
                &format!("[{mode}]   made Movable"),
                observe("movable", frame),
            );
            set_mesh_movability(&state, &id, ObjectMovability::Static);
            let frame = harness.frames(&mut renderer, || {});
            assert_drawn(
                &format!("[{mode}]   made Static again"),
                observe("static_again", frame),
            );

            move_to(&mut state, &id, [radius * 1.5, 0.0, 0.0]);
            let moved = harness.frames(&mut renderer, || {});
            let shift = moved.difference(&at_origin);
            assert_drawn(&format!("[{mode}]   moved"), observe("moved", moved));
            assert!(
                shift.depth_texels > 0,
                "[{mode}] moving the object did not move the mesh ({shift:?})"
            );

            set_visible(&mut state, &id, false);
            let frame = harness.frames(&mut renderer, || {});
            assert_not_drawn(
                &format!("[{mode}]   object hidden"),
                observe("hidden", frame),
            );
            set_visible(&mut state, &id, true);
            let frame = harness.frames(&mut renderer, || {});
            assert_drawn(&format!("[{mode}]   object shown"), observe("shown", frame));

            assert!(components::set_component_enabled(
                &mut state.scene.world_mut(),
                &id,
                0,
                false
            ));
            let frame = harness.frames(&mut renderer, || {});
            assert_not_drawn(
                &format!("[{mode}]   mesh disabled"),
                observe("disabled", frame),
            );
            assert!(components::set_component_enabled(
                &mut state.scene.world_mut(),
                &id,
                0,
                true
            ));
            let frame = harness.frames(&mut renderer, || {});
            assert_drawn(
                &format!("[{mode}]   mesh re-enabled"),
                observe("enabled", frame),
            );

            components::remove_component(&mut state.scene.world_mut(), &id, 0);
            let frame = harness.frames(&mut renderer, || {});
            assert_not_drawn(
                &format!("[{mode}]   mesh removed"),
                observe("removed", frame),
            );
        }

        // ── The properties panel subscribed to the mesh object ─────────────
        {
            let mut state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            let id = drop_mesh(&mut state);
            let feed = {
                let mut world = state.scene.world_mut();
                let entity = world.entity_for(&id).unwrap();
                pulsar_world_registry::ObjectFeed::subscribe(&mut world, entity, || {}).unwrap()
            };
            let frame = harness.frames(&mut renderer, || {
                feed.take();
            });
            assert_drawn(
                &format!("[{mode}] editor mesh, panel subscribed"),
                observe("panel_subscribed", frame),
            );
        }

        // ── Direct typed insertion after the first frame ───────────────────
        {
            let state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            {
                let mut world = state.scene.world_mut();
                let object = world
                    .spawn_object(engine_backend::scene::SpawnObject::new("typed mesh"))
                    .expect("spawn object");
                pulsar_world_registry::attach_value(&mut world, object, typed_mesh())
                    .expect("attach typed mesh");
            }
            let frame = harness.frames(&mut renderer, || {});
            assert_drawn(
                &format!("[{mode}] typed mesh, inserted directly"),
                observe("typed", frame),
            );
        }

        // ── Asset completion: geometry arrives after the instance ──────────
        {
            let mut state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            let id = add_object(
                &mut state,
                "pending mesh",
                ObjectType::Mesh(MeshType::Custom),
            );
            components::add_component(
                &mut state.scene.world_mut(),
                &id,
                "StaticMeshComponent".to_string(),
                json!({ "mesh_asset": "" }),
            );
            let frame = harness.frames(&mut renderer, || {});
            assert_not_drawn(
                &format!("[{mode}] mesh without geometry yet"),
                observe("pending", frame),
            );
            let instance = first_instance(&state, &id);
            {
                // The asset completes: the instance receives its geometry
                // and material, as an asset load writes them.
                let mut world = state.scene.world_mut();
                let completed = typed_mesh();
                world.insert(instance, completed);
            }
            let frame = harness.frames(&mut renderer, || {});
            assert_drawn(
                &format!("[{mode}]   geometry arrived"),
                observe("completed", frame),
            );
        }

        // ── Lights added after the first frame, next to a mesh ─────────────
        for (label, data, accepted) in [
            // The properties panel: Add Component, then the intensity edit.
            ("panel add", None, true),
            (
                "class to_json",
                Some(bright(class_json("LightComponent"))),
                true,
            ),
            ("legacy flat intensity", Some(legacy_flat_light()), false),
        ] {
            let mut state = LevelEditorState::new();
            drop_matte_mesh(&mut state);
            let mut renderer = harness.renderer(&state);
            let unlit = harness.frames(&mut renderer, || {});
            let id = add_object(&mut state, "Light", ObjectType::Light(LightType::Point));
            // Above and in front of the mesh, between it and the camera.
            move_to(&mut state, &id, [0.0, radius * 1.5, radius * 2.0]);
            match data {
                Some(data) => {
                    components::add_component(
                        &mut state.scene.world_mut(),
                        &id,
                        "LightComponent".to_string(),
                        data,
                    );
                }
                None => {
                    for command in [
                        SceneCommand::AddComponent {
                            id: id.clone(),
                            class_name: "LightComponent".into(),
                            value: None,
                        },
                        SceneCommand::SetComponentProperty {
                            id: id.clone(),
                            class_name: "LightComponent".into(),
                            component_index: 0,
                            prop_name: "intensity".into(),
                            value: Box::new(BRIGHT),
                        },
                    ] {
                        assert!(execute_command(&mut state, command).changed);
                    }
                }
            }
            let attached = components::instance_at(&state.scene.world(), &id, 0).is_some();
            assert_eq!(attached, accepted, "[{mode}] light ({label}): attached");
            let lit = harness.frames(&mut renderer, || {});
            let tag = format!("light_{}", label.replace(' ', "_"));
            lit.dump(&format!("{mode}_{tag}"));
            let change = lit.difference(&unlit);
            println!("PHASE2 [{mode}] light ({label}) vs unlit: {change:?}");
            if accepted {
                assert!(
                    change.color_pixels > 0,
                    "[{mode}] light ({label}) did not light the scene"
                );
                assert!(components::set_component_enabled(
                    &mut state.scene.world_mut(),
                    &id,
                    0,
                    false
                ));
                let dark = harness.frames(&mut renderer, || {});
                let change = dark.difference(&unlit);
                println!("PHASE2 [{mode}]   light disabled vs unlit: {change:?}");
                assert!(
                    change.color_pixels < (SIZE * SIZE / 100) as usize,
                    "[{mode}] a disabled light still lights the scene"
                );
            } else {
                assert_eq!(
                    change.color_pixels, 0,
                    "[{mode}] a refused light changed the frame"
                );
            }
        }
    }
}

/// Pulsar-Native#1035, Phase 4: fog media, post-process volumes and a
/// camera's post-process baseline, added through the editor's command path
/// as component instances, change the rendered frame through the full
/// default graph, and stop changing it when their instance is disabled.
/// Phase 5 (Pulsar-Native#1035): the scene lifecycle needs no repair path.
/// Undo and redo, replacing the level, and a second viewport on the same
/// scene all reach the frame through the ordinary revision step -- there is
/// no forced resync to call.
#[test]
fn history_level_replacement_and_viewports_need_no_resync() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let harness = Harness::new(device, queue, typed_mesh().bounds_local[3]);
    let reference = {
        let state = LevelEditorState::new();
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {})
    };
    let observe = |name: &str, frame: Frame| {
        frame.dump(&format!("lifecycle_{name}"));
        frame.difference(&reference)
    };

    // ── Undo and redo restore the world in place ───────────────────────
    {
        let mut state = LevelEditorState::new();
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {});
        let id = drop_mesh(&mut state);
        assert_drawn(
            "placed",
            observe("placed", harness.frames(&mut renderer, || {})),
        );

        execute_command(&mut state, SceneCommand::RemoveObject { id: id.clone() });
        assert_not_drawn(
            "removed",
            observe("removed", harness.frames(&mut renderer, || {})),
        );
        assert!(state.scene.undo());
        assert_drawn(
            "undo restores it",
            observe("undo", harness.frames(&mut renderer, || {})),
        );
        assert!(state.scene.redo());
        assert_not_drawn(
            "redo removes it",
            observe("redo", harness.frames(&mut renderer, || {})),
        );
    }

    // ── Opening a level into the running editor ────────────────────────
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mesh.level");
        {
            let mut authored = LevelEditorState::new();
            drop_mesh(&mut authored);
            ui_level_editor::scene_edit::level_io::save_to_file(&authored.scene.world(), &path)
                .unwrap();
        }
        let mut state = LevelEditorState::new();
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {});
        ui_level_editor::scene_edit::level_io::load_from_file(&mut state.scene.world_mut(), &path)
            .unwrap();
        assert_drawn(
            "opened level",
            observe("opened", harness.frames(&mut renderer, || {})),
        );

        let empty = dir.path().join("empty.level");
        ui_level_editor::scene_edit::level_io::save_to_file(
            &LevelEditorState::new().scene.world(),
            &empty,
        )
        .unwrap();
        ui_level_editor::scene_edit::level_io::load_from_file(&mut state.scene.world_mut(), &empty)
            .unwrap();
        assert_not_drawn(
            "replaced by an empty level",
            observe("replaced", harness.frames(&mut renderer, || {})),
        );
    }

    // ── Two viewports on one scene ─────────────────────────────────────
    {
        let mut state = LevelEditorState::new();
        let mut first = harness.renderer(&state);
        harness.frames(&mut first, || {});
        let id = drop_mesh(&mut state);
        assert_drawn(
            "first viewport",
            observe("first", harness.frames(&mut first, || {})),
        );
        let mut second = harness.renderer(&state);
        assert_drawn(
            "a viewport opened later",
            observe("second", harness.frames(&mut second, || {})),
        );
        assert_drawn(
            "first viewport, after the second opened",
            observe("first_again", harness.frames(&mut first, || {})),
        );
        execute_command(&mut state, SceneCommand::RemoveObject { id });
        assert_not_drawn(
            "removal, second viewport first",
            observe("second_removed", harness.frames(&mut second, || {})),
        );
        assert_not_drawn(
            "removal, first viewport after",
            observe("first_removed", harness.frames(&mut first, || {})),
        );
    }
}

#[test]
fn environment_components_reach_the_frame() {
    use helio_component::components::{
        CameraPostProcessComponent, GlobalFogComponent, LocalFogVolumeComponent,
        PostProcessVolumeComponent, WaterVolumeComponent,
    };
    use ui_level_editor::commands::TypedComponent;

    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);

    let mut fog = GlobalFogComponent::default();
    fog.medium.extinction = 0.02;
    fog.medium.emission = [0.5, 0.5, 0.5];
    let mut local = LocalFogVolumeComponent::default();
    // A dense box around the mesh only; the camera stays outside it.
    local.size = [radius * 3.0; 3];
    local.medium.extinction = 2.0;
    local.medium.emission = [4.0, 4.0, 4.0];
    let mut volume = PostProcessVolumeComponent::default();
    volume.unbound = true;
    volume.blend_weight = 1.0;
    volume.overrides.override_exposure = true;
    volume.settings.exposure_compensation = 3.0;
    let mut camera = CameraPostProcessComponent::default();
    camera.view_id = 0;
    camera.settings.exposure_compensation = -3.0;
    let mut water = WaterVolumeComponent::default();
    // A pool around the mesh, its surface through the mesh's centre.
    water.size = [radius * 3.0; 3];

    let cases: Vec<(&str, TypedComponent)> = vec![
        ("global fog", TypedComponent::new(fog)),
        ("local fog volume", TypedComponent::new(local)),
        ("post-process volume", TypedComponent::new(volume)),
        ("camera post-process", TypedComponent::new(camera)),
        ("water volume", TypedComponent::new(water)),
    ];
    for (label, component) in cases {
        let mut state = LevelEditorState::new();
        drop_mesh(&mut state);
        let mut renderer = harness.renderer(&state);
        let before = harness.frames(&mut renderer, || {});
        let added = execute_command(
            &mut state,
            SceneCommand::AddObjectWithComponents {
                data: SceneObjectData {
                    id: String::new(),
                    name: label.to_string(),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
                components: vec![component],
            },
        );
        let id = added.affected_ids[0].clone();
        let with = harness.frames(&mut renderer, || {});
        let tag = label.replace(' ', "_");
        with.dump(&format!("environment_{tag}"));
        let change = with.difference(&before);
        println!("PHASE4 {label} vs without: {change:?}");
        assert!(
            change.color_pixels > (SIZE * SIZE / 20) as usize,
            "{label} did not change the frame: {change:?}"
        );
        assert_eq!(change.depth_texels, 0, "{label} moved geometry");

        if label == "local fog volume" || label == "water volume" {
            // A volume follows its owner: moved far off-screen, the frame
            // is the frame without it.
            move_to(&mut state, &id, [radius * 100.0, 0.0, 0.0]);
            let away = harness.frames(&mut renderer, || {});
            let change = away.difference(&before);
            println!("PHASE4   {label} moved away vs without: {change:?}");
            assert!(
                change.color_pixels < (SIZE * SIZE / 100) as usize,
                "{label} still fogs the view after its owner moved away: {change:?}"
            );
            move_to(&mut state, &id, [0.0; 3]);
        }

        assert!(components::set_component_enabled(
            &mut state.scene.world_mut(),
            &id,
            0,
            false
        ));
        let disabled = harness.frames(&mut renderer, || {});
        let change = disabled.difference(&before);
        println!("PHASE4   {label} disabled vs without: {change:?}");
        assert!(
            change.color_pixels < (SIZE * SIZE / 100) as usize,
            "{label} still changes the frame when disabled: {change:?}"
        );
    }
}

/// Foliage reaches the foliage passes: grass grows on the ground plane
/// inside its owner's layer, and goes away when the layer moves away or the
/// component is disabled (Phase 4). The scene has no light, so blades are
/// checked by the depth they write. Blades are half a metre tall, so this
/// camera stands 4 m from the origin, 0.7 m up.
#[test]
fn foliage_reaches_the_frame() {
    use helio_component::components::FoliageComponent;
    use ui_level_editor::commands::TypedComponent;

    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let harness = Harness::new(device, queue, 1.0);
    // Placement fills a bounded number of tiles per frame, nearest first,
    // and the camera's ring holds a few hundred: give it time to settle.
    let settled = |renderer: &mut HelioRenderer| {
        for _ in 0..5 {
            harness.frames(renderer, || {});
        }
        harness.frames(renderer, || {})
    };

    let mut state = LevelEditorState::new();
    let mut renderer = harness.renderer(&state);
    let before = harness.frames(&mut renderer, || {});
    let mut foliage = FoliageComponent::default();
    foliage.general.enabled = true;
    foliage.placement.layer_extent = 20.0;
    let added = execute_command(
        &mut state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: "meadow".to_string(),
                object_type: ObjectType::Empty,
                transform: Transform::default(),
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            parent_id: None,
            components: vec![TypedComponent::new(foliage)],
        },
    );
    let id = added.affected_ids[0].clone();
    let with = settled(&mut renderer);
    with.dump("environment_foliage");
    let change = with.difference(&before);
    println!("PHASE4 foliage vs without: {change:?}");
    // The scene has no light, so the blades shade dark: depth is what shows
    // them drawn.
    assert!(
        change.depth_texels > (SIZE * SIZE / 20) as usize,
        "foliage drew no blades: {change:?}"
    );

    move_to(&mut state, &id, [1000.0, 0.0, 0.0]);
    let away = settled(&mut renderer);
    let change = away.difference(&before);
    println!("PHASE4   foliage moved away vs without: {change:?}");
    assert_eq!(
        change.depth_texels, 0,
        "grass still grows after its layer moved away: {change:?}"
    );
    move_to(&mut state, &id, [0.0; 3]);

    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &id,
        0,
        false
    ));
    let disabled = settled(&mut renderer);
    let change = disabled.difference(&before);
    println!("PHASE4   foliage disabled vs without: {change:?}");
    assert_eq!(
        change.depth_texels, 0,
        "grass still grows when disabled: {change:?}"
    );
}

