//! Render acceptance: meshes
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
//! This replaces the earlier read-only baseline (`11-phase-0-closure.md`
//! records its table): the cases it printed are assertions here. Run with
//! `cargo test -p ui_level_editor --test render_acceptance -- --nocapture`;
//! set `RENDER_ACCEPTANCE_DUMP_DIR` to also write every observed frame as a
//! PNG.

use std::collections::HashMap;

use engine_backend::scene::SceneWorldExt;
// Every installed plugin registers with the renderer, as in the editor.
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use helio_component::components::ObjectMovability;
use installed_plugins as _;
use installed_plugins::meshes_plugin::meshes_component::components::{
    LODLevel, MeshAssetPath, MeshGeometryKey, StaticMeshComponent, StaticMeshMaterialSlot,
    StaticMeshMaterialSlots,
};
use pulsar_reflection::REGISTRY;
use pulsar_scenedb::Entity;
use serde_json::{json, Value};
use ui_level_editor::commands::{execute_command, SceneCommand};
use ui_level_editor::scene_edit::components;
use ui_level_editor::scene_edit::{ObjectType, Transform};
use ui_level_editor::{LevelEditorState, SceneObjectData};

const SIZE: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Frames rendered per observation, so uploads, the object batch's async
/// draw-count readback and temporal filters settle. `RENDER_ACCEPTANCE_SETTLE_FRAMES`
/// overrides it.

/// The pipeline a level that names none runs: Helio's old default id, which
/// the default pipelines plugin's migration makes its deferred graph
/// (`plugins/pulsar.default_pipelines/default/Deferred.rendergraph`, made for the open
/// project's plugins).
///
/// Pipelines are project files, so a test that opened no project opens this
/// crate as one first (as the tests that hydrate meshes do).
fn shipped_pipeline() -> String {
    use engine_backend::subsystems::render::helio_renderer::pipelines;
    if engine_state::get_project_path().is_none() {
        engine_state::EngineContext::new().set_global();
        engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    }
    pipelines::migrate_pipeline_id(pipelines::LEGACY_DEFAULT_PIPELINE.to_string())
}

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
        .join("../../assets/meshes/primitives/SM_Cube.fbx")
        .canonicalize()
        .expect("SM_Cube.fbx is checked in");
    // Windows canonical paths carry a `\\?\` prefix the asset resolver rejects.
    let path = path.to_string_lossy();
    path.strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .replace('\\', "/")
}

fn mesh_asset_named(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/meshes/primitives")
        .join(name)
        .canonicalize()
        .expect("primitive mesh is checked in");
    let path = path.to_string_lossy();
    path.strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .replace('\\', "/")
}

/// One observed frame: final color and Helio's scene depth.
struct Frame {
    color: Vec<u8>,
    depth: Vec<f32>,
    /// Texels per row of `depth` (the scene depth is at the render
    /// resolution, not necessarily the target's).
    depth_width: u32,
}

/// How a frame differs from the empty-scene reference.
/// Whether two depth texels differ: relative, as reversed depth (near /
/// distance) is as precise a kilometre away as a metre away.
fn depth_differs(a: f32, b: f32) -> bool {
    (a - b).abs() > 1e-4 * a.abs().max(b.abs())
}

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
            .filter(|(a, b)| depth_differs(**a, **b))
            .count();
        Difference {
            color_pixels,
            depth_texels,
        }
    }

    /// With `RENDER_ACCEPTANCE_DUMP_DIR` set, writes where this frame's depth
    /// differs from `reference` (white) as `<name>.png`.
    fn dump_depth_change(&self, reference: &Frame, name: &str) {
        let Some(dir) = std::env::var_os("RENDER_ACCEPTANCE_DUMP_DIR") else {
            return;
        };
        let side = (self.depth.len() as f64).sqrt() as u32;
        let mask: Vec<u8> = reference
            .depth
            .iter()
            .zip(&self.depth)
            .map(|(a, b)| if depth_differs(*a, *b) { 255 } else { 0 })
            .collect();
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        image::save_buffer(&path, &mask, side, side, image::ColorType::L8).expect("write dump");
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
    /// camera change rebuilds Helio's Hi-Z pyramid; a camera
    /// at rest once culled everything (Helio `0e01e9fd`, since fixed),
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
            .editor_mailbox()
            .set_render_pipeline(Some(shipped_pipeline()));
        renderer
    }

    /// Renders `settle_frames()` frames, running `ui_frame` before each one as
    /// the UI thread would, and reads back the last one.
    fn frames(&self, renderer: &mut HelioRenderer, ui_frame: impl FnMut()) -> Frame {
        self.frames_at_least(renderer, ui_frame, 0)
    }

    /// [`Self::frames`], with at least `least` frames: a scene whose far side only shows through
    /// a portal needs more of them to settle than the others (its geometry binds over several
    /// frames under the asset pump's budget).
    fn frames_at_least(
        &self,
        renderer: &mut HelioRenderer,
        mut ui_frame: impl FnMut(),
        least: usize,
    ) -> Frame {
        let view = self.texture.create_view(&Default::default());
        let mut encoded = false;
        let frames = settle_frames().max(least).max(2) & !1;
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
            depth_width: depth.width(),
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
    let id = add_object(state, "SM_Cube", ObjectType::Empty);
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
fn emissive_surface(
) -> installed_plugins::meshes_plugin::meshes_component::mesh_cache::ImportedSurfaceMaterial {
    installed_plugins::meshes_plugin::meshes_component::mesh_cache::ImportedSurfaceMaterial {
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
        installed_plugins::meshes_plugin::meshes_component::mesh_files::load_mesh_asset_upload(
            std::path::Path::new(&mesh_asset()),
        )
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
        // Filled in here rather than bound from a load, so it names its
        // geometry itself.
        geometry_key: MeshGeometryKey::for_path(&MeshAssetPath::new(mesh_asset())),
        material_slots: StaticMeshMaterialSlots { slots },
        vertices: upload.geometry.vertices.into(),
        indices: upload.geometry.indices.into(),
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
            position: Some(position.map(f64::from)),
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
                depth_width: placed.depth_width,
            };
            assert_drawn(
                &format!("[{mode}] editor mesh, after the first frame"),
                observe("after_first_frame", placed),
            );

            set_mesh_movability(&state, &id, ObjectMovability::Movable);
            let frame = harness.frames(&mut renderer, || {});
            if !nudge {
                let appearance = frame.difference(&at_origin);
                assert_eq!(
                    appearance.depth_texels, 0,
                    "changing movability changed mesh depth/geometry: {appearance:?}"
                );
                assert!(
                    appearance.color_pixels < (SIZE * SIZE / 100) as usize,
                    "changing movability changed unshadowed mesh appearance: {appearance:?}"
                );
            }
            assert_drawn(
                &format!("[{mode}]   made Movable"),
                observe("movable", frame),
            );
            set_mesh_movability(&state, &id, ObjectMovability::Static);
            let frame = harness.frames(&mut renderer, || {});
            if !nudge {
                let appearance = frame.difference(&at_origin);
                assert_eq!(
                    appearance.depth_texels, 0,
                    "restoring Static changed mesh depth/geometry: {appearance:?}"
                );
                assert!(
                    appearance.color_pixels < (SIZE * SIZE / 100) as usize,
                    "restoring Static changed unshadowed mesh appearance: {appearance:?}"
                );
            }
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
            let id = add_object(&mut state, "pending mesh", ObjectType::Empty);
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
            let id = add_object(&mut state, "Light", ObjectType::Empty);
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

/// Fog media, post-process volumes and a
/// camera's post-process baseline, added through the editor's command path
/// as component instances, change the rendered frame through the full
/// default graph, and stop changing it when their instance is disabled.
/// The scene lifecycle needs no repair path.
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
        let state = LevelEditorState::new();
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
    use installed_plugins::fog_plugin::fog_component::{
        GlobalFogComponent, LocalFogVolumeComponent,
    };
    use installed_plugins::post_plugin::post_component::{
        CameraPostProcessComponent, PostProcessVolumeComponent,
    };
    use installed_plugins::water_plugin::water_component::WaterVolumeComponent;
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
    // A pool around the mesh, its surface through the mesh's centre: a
    // bounded volume (an ocean reaches the horizon wherever its owner is).
    water.size = [radius * 3.0; 3];
    water.bounded = true;

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
        before.dump(&format!("environment_{}_before", label.replace(' ', "_")));
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
            change.color_pixels > (SIZE * SIZE / 100) as usize,
            "{label} did not change the frame: {change:?}"
        );
        assert_eq!(change.depth_texels, 0, "{label} moved geometry");

        if label == "local fog volume" || label == "water volume" {
            // A volume follows its owner: moved far off-screen, the frame
            // is the frame without it.
            move_to(&mut state, &id, [radius * 100.0, 0.0, 0.0]);
            let away = harness.frames(&mut renderer, || {});
            away.dump(&format!("environment_{tag}_away"));
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
        disabled.dump(&format!("environment_{tag}_disabled"));
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
/// component is disabled. The scene has no light, so blades are
/// checked by the depth they write. Blades are half a metre tall, so this
/// camera stands 4 m from the origin, 0.7 m up.
#[test]
fn foliage_reaches_the_frame() {
    use installed_plugins::foliage_plugin::foliage_component::FoliageComponent;
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
    with.dump_depth_change(&before, "environment_foliage_depth");
    let change = with.difference(&before);
    println!("PHASE4 foliage vs without: {change:?}");
    // The scene has no light, so the blades shade dark: depth is what shows
    // them drawn, over 5% of the depth buffer (render resolution: under TSR
    // it is smaller than the output).
    assert!(
        change.depth_texels > with.depth.len() / 20,
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

/// Every pass of the editor's render graph
/// runs on a scene with a mesh and a light. The passes fed by authored
/// components are checked for their visible effect by the tests above; this
/// covers the engine-internal ones (culling, Hi-Z, shadows' bookkeeping,
/// post work) that have no scene input of their own.
#[test]
fn every_pass_of_the_editor_graph_runs() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let harness = Harness::new(device, queue, typed_mesh().bounds_local[3]);
    let mut state = LevelEditorState::new();
    drop_mesh(&mut state);
    let mut renderer = harness.renderer(&state);
    harness.frames(&mut renderer, || {});

    let activity = renderer.debug_pass_activity();
    let idle: Vec<&str> = activity
        .iter()
        .filter(|(_, ran)| !ran)
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(idle.is_empty(), "passes that recorded no work: {idle:?}");
    for expected in [
        "ObjectBatch",
        "ShadowMatrix",
        "ShadowDirty",
        "Shadow",
        "IndirectDispatch",
        "HiZBuild",
        "OcclusionCull",
        "LightCull",
        "GBuffer",
        "DeferredLight",
        "VgStream",
        "VgShade",
        "TransparentPass",
        "FXAA",
        "Lens",
        "LensFlare",
        "PostProcess",
        "DebugDraw",
        // Not the performance overlay: since the scene-driven graph it is a
        // member only while one of its modes is on (it is off here).
    ] {
        assert!(
            activity.iter().any(|(name, _)| name == expected),
            "{expected} is not in the editor graph: {activity:?}"
        );
    }
}

/// Sum of a frame's RGB: how much light reaches the camera.
fn brightness(frame: &Frame) -> u64 {
    frame
        .color
        .chunks_exact(4)
        .map(|px| px[0] as u64 + px[1] as u64 + px[2] as u64)
        .sum()
}

fn transform(
    state: &mut LevelEditorState,
    id: &str,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
) {
    let result = execute_command(
        state,
        SceneCommand::SetTransform {
            id: id.to_string(),
            position: None,
            rotation,
            scale,
        },
    );
    assert!(result.changed, "transform edit was refused");
}

/// Add a light at `position` through the properties panel's path (Add
/// Component, then reflected property edits) and return its object id.
fn add_light(
    state: &mut LevelEditorState,
    position: [f32; 3],
    light_type: installed_plugins::lights_plugin::lights_component::LightType,
    intensity: f32,
) -> String {
    let id = add_object(state, "Light", ObjectType::Empty);
    move_to(state, &id, position);
    assert!(
        execute_command(
            state,
            SceneCommand::AddComponent {
                id: id.clone(),
                class_name: "LightComponent".into(),
                value: None,
            },
        )
        .changed
    );
    set_light(state, &id, "light_type", Box::new(light_type));
    set_light(state, &id, "intensity", Box::new(intensity));
    id
}

#[test]
fn selected_light_guides_draw_and_clear_in_the_editor_frame() {
    use installed_plugins::lights_plugin::lights_component::LightType;

    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    let mut state = LevelEditorState::new();
    let mut lights = Vec::new();
    for (x, kind) in [(-radius, LightType::Point), (radius, LightType::Spot)] {
        let id = add_light(&mut state, [x, 0.0, 0.0], kind, 0.0);
        set_light(&mut state, &id, "range", Box::new(radius * 0.65));
        set_light(&mut state, &id, "cast_shadows", Box::new(false));
        lights.push(id);
    }

    state.scene.world_mut().select(None);
    let mut renderer = harness.renderer(&state);
    let clear = harness.frames(&mut renderer, || {});
    let selected: Vec<_> = lights
        .iter()
        .map(|id| state.scene.world().entity_for(id).expect("light owner"))
        .collect();
    state.scene.world_mut().select_all(&selected);
    let both = harness.frames(&mut renderer, || {});
    // Both halves contain a line layer. The icons are present in both frames.
    let mut changed = [0; 2];
    for (i, (a, b)) in both
        .color
        .chunks_exact(4)
        .zip(clear.color.chunks_exact(4))
        .enumerate()
    {
        let delta: u32 = (0..3).map(|c| u32::from(a[c].abs_diff(b[c]))).sum();
        if delta > PIXEL_THRESHOLD {
            changed[usize::from(i % SIZE as usize >= SIZE as usize / 2)] += 1;
        }
    }
    assert!(changed.iter().all(|&count| count > 20), "{changed:?}");

    state.scene.world_mut().select(None);
    let cleared = harness.frames(&mut renderer, || {});
    assert_eq!(cleared.difference(&clear).color_pixels, 0);
}

fn set_light(
    state: &mut LevelEditorState,
    id: &str,
    property: &str,
    value: Box<dyn std::any::Any + Send>,
) {
    let result = execute_command(
        state,
        SceneCommand::SetComponentProperty {
            id: id.to_string(),
            class_name: "LightComponent".into(),
            component_index: 0,
            prop_name: property.into(),
            value,
        },
    );
    assert!(result.changed, "light {property} edit was refused");
}

/// Authored mesh LOD selects a different mesh asset from the active render
/// view. Removing the policy by disabling its component returns the ordinary
/// StaticMeshComponent draw ranges unchanged.
#[test]
fn authored_mesh_lod_changes_with_distance_and_bias_and_disabling_restores_base_mesh() {
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
    let near_distance = glam::DVec3::from_array(harness.camera.position).length() as f32;
    let mut state = LevelEditorState::new();
    let id = drop_mesh(&mut state);

    let add_lod = execute_command(
        &mut state,
        SceneCommand::AddComponent {
            id: id.clone(),
            class_name: "LODComponent".into(),
            value: None,
        },
    );
    assert!(add_lod.changed, "LOD component insertion was refused");
    let levels_edit = execute_command(
        &mut state,
        SceneCommand::SetComponentProperty {
            id: id.clone(),
            class_name: "LODComponent".into(),
            component_index: 1,
            prop_name: "lod_levels".into(),
            value: Box::new(vec![
                LODLevel {
                    distance_threshold: 0.0,
                    mesh_asset: MeshAssetPath::new(mesh_asset()),
                },
                LODLevel {
                    distance_threshold: near_distance * 1.5,
                    mesh_asset: MeshAssetPath::new(mesh_asset_named("SM_Sphere.fbx")),
                },
            ]),
        },
    );
    assert!(levels_edit.changed, "LOD level edit was refused");
    let disable_for_base = execute_command(
        &mut state,
        SceneCommand::SetComponentEnabled {
            id: id.clone(),
            component_index: 1,
            enabled: false,
        },
    );
    assert!(
        disable_for_base.changed,
        "disabling initial LOD component failed"
    );
    let mut renderer = harness.renderer(&state);
    let base_mesh = harness.frames(&mut renderer, || {});
    let enable_for_lod = execute_command(
        &mut state,
        SceneCommand::SetComponentEnabled {
            id: id.clone(),
            component_index: 1,
            enabled: true,
        },
    );
    assert!(
        enable_for_lod.changed,
        "enabling initial LOD component failed"
    );

    // At the default camera distance, level zero names the base cube, so the
    // authored selector should leave the rendered frame equivalent to the
    // ordinary mesh path.
    let near_lod = harness.frames(&mut renderer, || {});
    assert!(
        near_lod.difference(&base_mesh).depth_texels < 8
            && near_lod.difference(&base_mesh).color_pixels < (SIZE * SIZE / 100) as usize,
        "the near authored level should preserve the base mesh ({:?})",
        near_lod.difference(&base_mesh)
    );

    // A farther camera crosses the second level's threshold and selects the
    // sphere. Compare against the base mesh at the same view so the test
    // isolates mesh selection from the camera's changed projection.
    let far = harness.camera.position[2] * 2.0;
    renderer.set_editor_camera_state(EditorCameraState {
        position: [0.0, far / 6.0, far],
        ..harness.camera
    });
    let far_lod = harness.frames(&mut renderer, || {});
    let disable_for_control = execute_command(
        &mut state,
        SceneCommand::SetComponentEnabled {
            id: id.clone(),
            component_index: 1,
            enabled: false,
        },
    );
    assert!(disable_for_control.changed, "disabling LOD control failed");
    let far_base = harness.frames(&mut renderer, || {});
    let enable_for_bias = execute_command(
        &mut state,
        SceneCommand::SetComponentEnabled {
            id: id.clone(),
            component_index: 1,
            enabled: true,
        },
    );
    assert!(enable_for_bias.changed, "re-enabling LOD component failed");
    let distance_change = far_lod.difference(&far_base);
    assert!(
        distance_change.depth_texels > 0 && distance_change.color_pixels > 0,
        "crossing the authored distance threshold did not change the drawn mesh: {distance_change:?}"
    );

    // Return to the near view and bias the distance threshold so level one is
    // selected sooner; then restore the bias and disable the component.
    renderer.set_editor_camera_state(harness.camera);
    let bias_edit = execute_command(
        &mut state,
        SceneCommand::SetComponentProperty {
            id: id.clone(),
            class_name: "LODComponent".into(),
            component_index: 1,
            prop_name: "lod_bias".into(),
            value: Box::new(2.0_f32),
        },
    );
    assert!(bias_edit.changed, "LOD bias edit was refused");
    let bias_value = components::read_live_component_property(
        &state.scene.world(),
        &id,
        "LODComponent",
        1,
        "lod_bias",
    )
    .and_then(|value| value.downcast::<f32>().ok())
    .map(|value| *value);
    assert_eq!(bias_value, Some(2.0));
    let biased_lod = harness.frames(&mut renderer, || {});
    let bias_change = biased_lod.difference(&near_lod);
    assert!(
        bias_change.depth_texels > 0 && bias_change.color_pixels > 0,
        "positive bias did not select the coarser authored mesh sooner: {bias_change:?}"
    );

    let disable = execute_command(
        &mut state,
        SceneCommand::SetComponentEnabled {
            id,
            component_index: 1,
            enabled: false,
        },
    );
    assert!(disable.changed, "disabling LOD component was refused");
    let disabled_lod = harness.frames(&mut renderer, || {});
    let fallback = disabled_lod.difference(&base_mesh);
    assert!(
        fallback.depth_texels < 8 && fallback.color_pixels < (SIZE * SIZE / 100) as usize,
        "disabling authored LOD did not restore the base mesh: {fallback:?}"
    );
}

/// Mesh/light regression: scale,
/// every light type, intensity, direction, a light's shadow casting, and the
/// inspector open or closed, through the editor's producers to the frame.
#[test]
fn mesh_and_light_variations_reach_the_frame() {
    use installed_plugins::lights_plugin::lights_component::LightType as Kind;
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
    let reference = {
        let state = LevelEditorState::new();
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {})
    };

    // ── Scale: a smaller mesh covers less of the frame ───────────────────
    {
        let mut state = LevelEditorState::new();
        let id = drop_mesh(&mut state);
        let mut renderer = harness.renderer(&state);
        let full = harness.frames(&mut renderer, || {}).difference(&reference);
        transform(&mut state, &id, None, Some([0.5, 0.5, 0.5]));
        let half = harness.frames(&mut renderer, || {}).difference(&reference);
        println!("PHASE7 scale: full {full:?}, half {half:?}");
        assert_drawn("scaled mesh", half);
        assert!(
            half.depth_texels * 2 < full.depth_texels,
            "half scale should cover well under half the texels ({half:?} vs {full:?})"
        );
    }

    // ── Light types, intensity and direction ──────────────────────────────
    let above = [0.0, radius * 1.5, radius * 2.0];
    for (kind, intensity) in [
        (Kind::Point, BRIGHT),
        (Kind::Spot, BRIGHT),
        (Kind::Directional, 20.0),
    ] {
        let mut state = LevelEditorState::new();
        drop_matte_mesh(&mut state);
        let mut renderer = harness.renderer(&state);
        let unlit = harness.frames(&mut renderer, || {});
        // Lights shine along their owner's -Y: down, onto the mesh.
        let id = add_light(&mut state, above, kind, intensity);
        let lit = harness.frames(&mut renderer, || {});
        let change = lit.difference(&unlit);
        println!("PHASE7 {kind:?} light vs unlit: {change:?}");
        assert!(
            change.color_pixels > 0,
            "{kind:?} light did not light the scene"
        );

        set_light(&mut state, &id, "intensity", Box::new(intensity * 4.0));
        let brighter = harness.frames(&mut renderer, || {});
        assert!(
            brighter.difference(&lit).color_pixels > 0
                && brighter.difference(&unlit).color_pixels >= change.color_pixels,
            "{kind:?}: raising the intensity changed nothing"
        );
        if kind != Kind::Point {
            // Turned to shine straight up, away from the mesh.
            transform(&mut state, &id, Some([180.0, 0.0, 0.0]), None);
            let away = harness.frames(&mut renderer, || {});
            println!(
                "PHASE7 {kind:?} brightness: down {}, up {}",
                brightness(&brighter),
                brightness(&away)
            );
            assert!(
                brightness(&away) < brightness(&brighter),
                "{kind:?}: turning the light away did not darken the mesh"
            );
        }
    }

    // ── A light's shadow casting ──────────────────────────────────────────
    {
        let mut state = LevelEditorState::new();
        let floor = drop_matte_mesh(&mut state);
        move_to(&mut state, &floor, [0.0, -radius, 0.0]);
        transform(&mut state, &floor, None, Some([3.0, 0.05, 3.0]));
        let blocker = drop_matte_mesh(&mut state);
        transform(&mut state, &blocker, None, Some([0.4, 0.4, 0.4]));
        let id = add_light(&mut state, [0.0, radius * 2.5, 0.0], Kind::Point, BRIGHT);
        let mut renderer = harness.renderer(&state);
        let shadowed = harness.frames(&mut renderer, || {});
        shadowed.dump("light_shadows_on");
        set_light(&mut state, &id, "cast_shadows", Box::new(false));
        let unshadowed = harness.frames(&mut renderer, || {});
        unshadowed.dump("light_shadows_off");
        let change = unshadowed.difference(&shadowed);
        println!(
            "PHASE7 cast_shadows off vs on: {change:?}, brightness {} -> {}",
            brightness(&shadowed),
            brightness(&unshadowed)
        );
        assert!(
            change.color_pixels > 0,
            "turning the light's shadows off changed nothing"
        );
        assert!(
            brightness(&unshadowed) > brightness(&shadowed),
            "without shadows the floor under the blocker should be lit"
        );
    }

    // ── Inspector open or closed: the same frame ──────────────────────────
    {
        let scene = |state: &mut LevelEditorState| {
            drop_matte_mesh(state);
            add_light(state, above, Kind::Point, BRIGHT)
        };
        let mut closed_state = LevelEditorState::new();
        scene(&mut closed_state);
        let mut renderer = harness.renderer(&closed_state);
        let closed = harness.frames(&mut renderer, || {});

        let mut open_state = LevelEditorState::new();
        let id = scene(&mut open_state);
        let feed = {
            let mut world = open_state.scene.world_mut();
            let entity = world.entity_for(&id).unwrap();
            pulsar_world_registry::ObjectFeed::subscribe(&mut world, entity, || {}).unwrap()
        };
        let mut renderer = harness.renderer(&open_state);
        let open = harness.frames(&mut renderer, || {
            feed.take();
        });
        let change = open.difference(&closed);
        println!("PHASE7 inspector open vs closed: {change:?}");
        assert_eq!(change.depth_texels, 0, "the inspector changed the depth");
        assert!(
            change.color_pixels < (SIZE * SIZE / 200) as usize,
            "the inspector changed the frame ({change:?})"
        );
    }
}

/// Real render output: a spline added
/// through the editor is drawn by the editor debug pass, follows its owner,
/// and disappears when disabled or removed.
#[test]
fn splines_reach_the_frame() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let radius = 50.0;
    let harness = Harness::new(device, queue, radius);
    let mut state = LevelEditorState::new();
    let mut renderer = harness.renderer(&state);
    let reference = harness.frames(&mut renderer, || {});

    let id = add_object(&mut state, "Spline", ObjectType::Empty);
    let point = |position: [f32; 3]| json!({ "position": position });
    components::add_component(
        &mut state.scene.world_mut(),
        &id,
        "SplineComponent".to_string(),
        json!({
            "points": [
                point([-radius, 0.0, 0.0]),
                point([0.0, radius * 0.5, 0.0]),
                point([radius, 0.0, 0.0]),
            ],
        }),
    );
    assert!(
        components::instance_at(&state.scene.world(), &id, 0).is_some(),
        "the spline attached"
    );
    let drawn = harness.frames(&mut renderer, || {});
    reference.dump("spline_reference");
    drawn.dump("spline_drawn");
    let change = drawn.difference(&reference);
    println!("PHASE7 spline: {change:?}");
    assert!(
        change.color_pixels > 0,
        "the spline was not drawn ({change:?})"
    );

    move_to(&mut state, &id, [0.0, radius * 0.6, 0.0]);
    let moved = harness.frames(&mut renderer, || {});
    assert!(
        moved.difference(&drawn).color_pixels > 0,
        "the spline did not follow its owner"
    );

    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &id,
        0,
        false
    ));
    let disabled = harness.frames(&mut renderer, || {});
    assert!(
        disabled.difference(&reference).color_pixels < change.color_pixels / 4,
        "a disabled spline is still drawn"
    );
    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &id,
        0,
        true
    ));
    assert!(
        harness
            .frames(&mut renderer, || {})
            .difference(&reference)
            .color_pixels
            > 0
    );

    components::remove_component(&mut state.scene.world_mut(), &id, 0);
    let removed = harness.frames(&mut renderer, || {});
    assert!(
        removed.difference(&reference).color_pixels < change.color_pixels / 4,
        "a removed spline is still drawn"
    );
}

/// Cost and architecture: once a
/// scene settles, an unchanged scene encodes no frame and writes nothing to
/// the world, however often the viewport asks; one edit wakes the renderer,
/// which settles back to idle.
/// Depth texels that differ from `reference` in the left and right halves
/// of the frame.
fn depth_by_half(frame: &Frame, reference: &Frame) -> (usize, usize) {
    let mut halves = (0, 0);
    for (index, (a, b)) in reference.depth.iter().zip(&frame.depth).enumerate() {
        if depth_differs(*a, *b) {
            if (index as u32 % frame.depth_width) < frame.depth_width / 2 {
                halves.0 += 1;
            } else {
                halves.1 += 1;
            }
        }
    }
    halves
}

/// Which texels of one half of the frame differ in depth from `reference`
/// (what the object on that side covers).
fn coverage(frame: &Frame, reference: &Frame, left: bool) -> Vec<bool> {
    reference
        .depth
        .iter()
        .zip(&frame.depth)
        .enumerate()
        .filter(|(index, _)| ((*index as u32 % frame.depth_width) < frame.depth_width / 2) == left)
        .map(|(_, (a, b))| depth_differs(*a, *b))
        .collect()
}

/// Intersection over union of two coverages: 1 when they are the same
/// texels. Temporal jitter flips a few edge texels; a moved object shares
/// few of its texels with where it was.
fn overlap(a: &[bool], b: &[bool]) -> f32 {
    let both = a.iter().zip(b).filter(|(a, b)| **a && **b).count();
    let either = a.iter().zip(b).filter(|(a, b)| **a || **b).count();
    if either == 0 {
        1.0
    } else {
        both as f32 / either as f32
    }
}

/// A duplicate is a wholesale copy: in the frame, moving, hiding or
/// deleting either one leaves the other as it was. Both share their mesh
/// geometry on the GPU (content-interned); that must not tie them.
#[test]
fn duplicates_are_independent_in_the_frame() {
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
    let reference = {
        let state = LevelEditorState::new();
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {})
    };
    let left = [-0.9 * radius, 0.0, 0.0];
    let duplicate = |state: &mut LevelEditorState, source: &str| {
        execute_command(
            state,
            SceneCommand::DuplicateObject {
                source_id: source.to_string(),
                count: 1,
                position_offset: Some([1.8 * radius, 0.0, 0.0]),
            },
        )
        .affected_ids[0]
            .clone()
    };

    for remove_source in [true, false] {
        let mut state = LevelEditorState::new();
        let source = drop_mesh(&mut state);
        move_to(&mut state, &source, left);
        let copy = duplicate(&mut state, &source);
        let mut renderer = harness.renderer(&state);
        let both = harness.frames(&mut renderer, || {});
        let (l, r) = depth_by_half(&both, &reference);
        println!("DUPLICATE both: left {l}, right {r}");
        assert!(
            l > 0 && r > 0,
            "the source and the copy are drawn ({l}, {r})"
        );

        let source_texels = coverage(&both, &reference, true);
        let stays = |frame: &Frame, what: &str| {
            let same = overlap(&source_texels, &coverage(frame, &reference, true));
            println!("DUPLICATE {what}: the source keeps {same:.3} of its texels");
            assert!(same > 0.9, "{what} moved the source ({same:.3})");
        };
        // Moving the source leaves the copy where it was.
        let copy_texels = coverage(&both, &reference, false);
        move_to(&mut state, &source, [-0.9 * radius, 0.6 * radius, 0.0]);
        let source_moved = harness.frames(&mut renderer, || {});
        let same = overlap(&copy_texels, &coverage(&source_moved, &reference, false));
        println!("DUPLICATE moving the source: the copy keeps {same:.3} of its texels");
        assert!(same > 0.9, "moving the source moved the copy ({same:.3})");
        assert!(
            overlap(&source_texels, &coverage(&source_moved, &reference, true)) < 0.9,
            "the source did not move"
        );
        move_to(&mut state, &source, left);

        // Moving the copy up leaves the source where it was.
        both.dump(&format!("duplicate_{remove_source}_both"));
        move_to(&mut state, &copy, [0.9 * radius, 0.6 * radius, 0.0]);
        let moved = harness.frames(&mut renderer, || {});
        moved.dump(&format!("duplicate_{remove_source}_moved"));
        stays(&moved, "moving the copy");
        // Hiding the copy keeps the source.
        set_visible(&mut state, &copy, false);
        let hidden = harness.frames(&mut renderer, || {});
        stays(&hidden, "hiding the copy");
        assert_eq!(
            depth_by_half(&hidden, &reference).1,
            0,
            "the hidden copy is drawn"
        );
        set_visible(&mut state, &copy, true);
        move_to(&mut state, &copy, [0.9 * radius, 0.0, 0.0]);

        // Deleting either one keeps the other drawn: the shared geometry
        // stays while one of them holds it.
        let (gone, kept) = if remove_source {
            (&source, &copy)
        } else {
            (&copy, &source)
        };
        let result = execute_command(&mut state, SceneCommand::RemoveObject { id: gone.clone() });
        assert!(result.changed, "remove {gone}");
        let after = harness.frames(&mut renderer, || {});
        let (l2, r2) = depth_by_half(&after, &reference);
        println!("DUPLICATE removed {gone} (kept {kept}): left {l2}, right {r2}");
        if remove_source {
            assert_eq!((l2, r2 > 0), (0, true), "removing the source");
        } else {
            assert_eq!(r2, 0, "removing the copy");
            stays(&after, "removing the copy");
        }
    }
}

#[test]
fn an_idle_scene_encodes_nothing_and_an_edit_wakes_it() {
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
    let mut state = LevelEditorState::new();
    drop_matte_mesh(&mut state);
    let light = add_light(
        &mut state,
        [0.0, radius * 1.5, radius * 2.0],
        installed_plugins::lights_plugin::lights_component::LightType::Point,
        BRIGHT,
    );
    let mut renderer = harness.renderer(&state);
    harness.frames(&mut renderer, || {});

    let view = harness.texture.create_view(&Default::default());
    let encode = |renderer: &mut HelioRenderer| {
        let encoded = renderer
            .render_frame(&harness.device, &harness.queue, &view, SIZE, SIZE, FORMAT)
            .is_some();
        harness
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        encoded
    };
    // Settle whatever the forced frames left pending.
    let mut settled = false;
    for _ in 0..16 {
        if !encode(&mut renderer) {
            settled = true;
            break;
        }
    }
    assert!(settled, "the renderer never went idle");

    let revision = state.scene.world().revision();
    let idle: Vec<bool> = (0..20).map(|_| encode(&mut renderer)).collect();
    assert!(
        idle.iter().all(|encoded| !encoded),
        "an idle scene encoded a frame: {idle:?}"
    );
    assert_eq!(
        state.scene.world().revision(),
        revision,
        "idle frames wrote to the world"
    );

    set_light(&mut state, &light, "intensity", Box::new(BRIGHT * 2.0));
    assert!(encode(&mut renderer), "the edit did not wake the renderer");
    let after: Vec<bool> = (0..16).map(|_| encode(&mut renderer)).collect();
    let woken = after.iter().take_while(|encoded| **encoded).count();
    println!("PHASE7 frames after one edit before idle: {}", 1 + woken);
    assert!(
        after.iter().skip(woken).all(|encoded| !encoded),
        "the renderer did not settle back to idle: {after:?}"
    );
}

/// Mean RGB sum over the top quarter of a frame: above the horizon from the
/// harness camera, where only the sky can be.
fn sky_brightness(frame: &Frame) -> f64 {
    let rows = (SIZE / 4) as usize;
    let pixels = &frame.color[..rows * SIZE as usize * 4];
    let sum: u64 = pixels
        .chunks_exact(4)
        .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
        .sum();
    sum as f64 / (rows * SIZE as usize) as f64
}

/// The sky is the level's AtmosphereComponent: without one the sky
/// is black, even with a sun; World Settings' "Create Sky" adds one and the
/// sky lights up; disabling or removing it makes the sky black again.
#[test]
fn the_sky_is_the_levels_atmosphere() {
    use plugin_kinds::sky;

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

    let mut state = LevelEditorState::new();
    // A sun high overhead, its icon far outside the view.
    add_light(
        &mut state,
        [1.0e4, 0.0, 0.0],
        installed_plugins::lights_plugin::lights_component::LightType::Directional,
        SUN,
    );
    let mut renderer = harness.renderer(&state);
    let without = harness.frames(&mut renderer, || {});
    without.dump("sky_without_atmosphere");
    let black = sky_brightness(&without);

    let created = sky::create_sky(&mut state);
    assert!(created.changed, "{}", created.no_op_reason);
    let with = harness.frames(&mut renderer, || {});
    with.dump("sky_with_atmosphere");
    let lit = sky_brightness(&with);
    println!("SKY without atmosphere {black:.2}, with {lit:.2}");
    assert!(
        black < 1.0,
        "the sky is not black without an atmosphere: {black}"
    );
    assert!(lit > 30.0, "the atmosphere did not light the sky: {lit}");

    let found = sky::level_sky(&state.scene.world()).expect("the level has a sky");
    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &found.object_id,
        found.component_index,
        false
    ));
    let disabled = sky_brightness(&harness.frames(&mut renderer, || {}));
    println!("SKY disabled {disabled:.2}");
    assert!(
        disabled < 1.0,
        "a disabled atmosphere still lights the sky: {disabled}"
    );
}

/// Illuminance of the acceptance tests' sun (lux).
const SUN: f32 = 20.0;

/// Decals: an authored decal recolours the surfaces in its box and
/// stays (it is permanent); scripts set how it disappears through its
/// opacity: `DecalComponent::set_opacity` (its property accessor) and
/// `DecalComponent::fade_by` (its method), called as a script calls them.
#[test]
fn decals_reach_the_frame_and_scripts_set_their_opacity() {
    use installed_plugins::decals_plugin::decals_component::{DecalComponent, DecalLayers};
    use pulsar_script_vm::{Host, NativeRegistry, Value};
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

    // A matte white mesh under a light: the decal recolours its surface.
    let mut state = LevelEditorState::new();
    drop_matte_mesh(&mut state);
    add_light(
        &mut state,
        [0.0, radius * 1.5, radius * 2.0],
        installed_plugins::lights_plugin::lights_component::LightType::Point,
        BRIGHT,
    );
    let mut renderer = harness.renderer(&state);
    let before = harness.frames(&mut renderer, || {});

    // A blue decal whose box holds the whole mesh.
    let decal = DecalComponent {
        size: [radius * 3.0; 3],
        color: [0.0, 0.2, 1.0],
        layers: DecalLayers::Albedo,
        ..Default::default()
    };
    let added = execute_command(
        &mut state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: "Decal".to_string(),
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
            components: vec![TypedComponent::new(decal)],
        },
    );
    let id = added.affected_ids[0].clone();
    let with = harness.frames(&mut renderer, || {});
    with.dump("decal");
    let change = with.difference(&before);
    println!("DECAL vs without: {change:?}");
    assert!(
        change.color_pixels > (SIZE * SIZE / 50) as usize,
        "the decal did not change the frame: {change:?}"
    );
    assert_eq!(change.depth_texels, 0, "a decal moved geometry");
    // Permanent: still there frames later.
    let later = harness.frames(&mut renderer, || {});
    assert!(later.difference(&with).color_pixels < (SIZE * SIZE / 200) as usize);

    // Scripts address the decal as a component reference.
    let natives = NativeRegistry::with_engine_natives();
    let call = |state: &LevelEditorState, native: &str, argument: f64| -> Value {
        let mut world = state.scene.world_mut();
        let instance = components::instance_at(&world, &id, 0).expect("the decal");
        let owner = engine_backend::scene::SceneWorldExt::entity_for(&*world, &id).unwrap();
        let reference =
            Value::Component(pulsar_scenedb::ComponentRef::of::<DecalComponent>(instance));
        let mut host = Host::new(&mut world, owner);
        natives
            .get(native)
            .unwrap_or_else(|| panic!("{native} is a script native"))
            .call(&mut host, &mut [reference, Value::Float(argument)])
            .unwrap_or_else(|error| panic!("{native}: {error:?}"))
    };
    let opacity = |state: &LevelEditorState| {
        let world = state.scene.world();
        let instance = components::instance_at(&world, &id, 0).unwrap();
        world.get::<DecalComponent>(instance).unwrap().opacity
    };

    call(&state, "DecalComponent::set_opacity", 0.0);
    assert_eq!(opacity(&state), 0.0);
    let hidden = harness.frames(&mut renderer, || {});
    let change = hidden.difference(&before);
    println!("DECAL opacity 0 vs without: {change:?}");
    assert!(
        change.color_pixels < (SIZE * SIZE / 100) as usize,
        "a decal at opacity 0 still shows: {change:?}"
    );

    call(&state, "DecalComponent::set_opacity", 1.0);
    let left = call(&state, "DecalComponent::fade_by", 0.5);
    assert!(matches!(left, Value::Float(left) if (left - 0.5).abs() < 1e-6));
    assert_eq!(opacity(&state), 0.5);
    let half = harness.frames(&mut renderer, || {});
    let from_full = half.difference(&with);
    let from_none = half.difference(&before);
    println!("DECAL opacity 0.5 vs full {from_full:?}, vs without {from_none:?}");
    assert!(
        from_full.color_pixels > 0 && from_none.color_pixels > 0,
        "half opacity is neither the full decal nor none"
    );
}

/// Changed pixels in one half of `frame` against `reference`, and their
/// mean red and green.
fn half_change(frame: &Frame, reference: &Frame, left: bool) -> (usize, f32, f32) {
    let (mut count, mut red, mut green) = (0usize, 0.0f32, 0.0f32);
    for (index, (a, b)) in reference
        .color
        .chunks_exact(4)
        .zip(frame.color.chunks_exact(4))
        .enumerate()
    {
        if ((index as u32 % SIZE) < SIZE / 2) != left {
            continue;
        }
        if (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > PIXEL_THRESHOLD {
            count += 1;
            red += b[0] as f32;
            green += b[1] as f32;
        }
    }
    let n = count.max(1) as f32;
    (count, red / n, green / n)
}

/// Particle emitters: an authored emitter's particles reach the
/// frame; two emitters with different `max_particles` take their own
/// ranges of the shared particle pool and both draw; disabling one
/// restores its side of the frame while the other keeps drawing.
#[test]
fn particle_emitters_reach_the_frame_from_one_pool() {
    use installed_plugins::particles_plugin::particles_component::{
        ParticleEmitterComponent, ParticleEmitterShape,
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
    let harness = Harness::new(device, queue, 1.0);
    // No atmosphere: a black sky, so the particles alone change the frame.
    let mut state = LevelEditorState::new();
    let mut renderer = harness.renderer(&state);
    let before = harness.frames(&mut renderer, || {});

    // Long-lived, still particles inside a sphere around their owner.
    let add_emitter = |state: &mut LevelEditorState, x: f32, color: [f32; 4], max: u32| {
        let emitter = ParticleEmitterComponent {
            max_particles: max,
            emit_rate: 20_000.0,
            lifetime: 60.0,
            lifetime_variation: 0.0,
            shape: ParticleEmitterShape::Sphere,
            spawn_radius: 0.7,
            velocity: [0.0; 3],
            velocity_variation: [0.0; 3],
            start_size: 0.12,
            end_size: 0.12,
            start_color: color,
            end_color: color,
            ..Default::default()
        };
        let added = execute_command(
            state,
            SceneCommand::AddObjectWithComponents {
                data: SceneObjectData {
                    id: String::new(),
                    name: "Particles".to_string(),
                    object_type: ObjectType::Empty,
                    transform: Transform {
                        position: [f64::from(x), 0.5, 0.0],
                        ..Transform::default()
                    },
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
                components: vec![TypedComponent::new(emitter)],
            },
        );
        added.affected_ids[0].clone()
    };

    // A small red emitter on the left.
    let red = add_emitter(&mut state, -1.3, [1.0, 0.05, 0.05, 1.0], 300);
    let one = harness.frames(&mut renderer, || {});
    one.dump("particles_one");
    let left = half_change(&one, &before, true);
    let right = half_change(&one, &before, false);
    println!("PARTICLES one emitter: left {left:?}, right {right:?}");
    assert!(left.0 > 200, "the emitter drew no particles: {left:?}");
    assert!(left.1 > left.2 * 2.0, "the particles are not red: {left:?}");
    assert!(
        right.0 < 20,
        "particles drawn away from the emitter: {right:?}"
    );
    assert_eq!(
        one.difference(&before).depth_texels,
        0,
        "particles wrote depth"
    );

    // A larger green one on the right: its own range of the pool, both draw.
    let green = add_emitter(&mut state, 1.3, [0.05, 1.0, 0.05, 1.0], 3000);
    let two = harness.frames(&mut renderer, || {});
    two.dump("particles_two");
    let left = half_change(&two, &before, true);
    let right = half_change(&two, &before, false);
    println!("PARTICLES two emitters: left {left:?}, right {right:?}");
    assert!(
        left.0 > 200 && left.1 > left.2 * 2.0,
        "the red emitter stopped: {left:?}"
    );
    assert!(
        right.0 > 200 && right.2 > right.1 * 2.0,
        "the green emitter drew nothing: {right:?}"
    );
    assert!(
        right.0 > left.0,
        "ten times the particles cover no more of the frame: left {left:?}, right {right:?}"
    );

    // Disabling the red one restores its side; the green one now packs into
    // the first row and its first range of the pool, and keeps drawing.
    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &red,
        0,
        false
    ));
    let disabled = harness.frames(&mut renderer, || {});
    disabled.dump("particles_disabled");
    let left = half_change(&disabled, &before, true);
    let right = half_change(&disabled, &before, false);
    println!("PARTICLES red disabled: left {left:?}, right {right:?}");
    assert!(left.0 < 20, "a disabled emitter still draws: {left:?}");
    assert!(
        right.0 > 200 && right.2 > right.1 * 2.0,
        "the green emitter stopped: {right:?}"
    );

    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &green,
        0,
        false
    ));
    let none = harness.frames(&mut renderer, || {});
    let change = none.difference(&before);
    println!("PARTICLES both disabled: {change:?}");
    assert!(
        change.color_pixels < 20,
        "particles outlive their emitters: {change:?}"
    );
}

/// Edit the level's global wind in place, as its World Settings rows do.
fn set_global_wind(
    state: &mut LevelEditorState,
    edit: impl FnOnce(&mut installed_plugins::foliage_plugin::foliage_component::WindComponent),
) {
    let mut world = state.scene.world_mut();
    let (instance, _) = installed_plugins::foliage_plugin::foliage_component::level_wind(&world)
        .expect("the level has a wind");
    edit(
        &mut world
            .get_mut::<installed_plugins::foliage_plugin::foliage_component::WindComponent>(
                instance,
            )
            .unwrap(),
    );
}

/// Wind: foliage sways in the level's global wind, on the
/// renderer's frame clock. In a realtime viewport two observations with no
/// scene write differ while the wind blows; they are identical in a calm or
/// with Realtime off (the clock frozen). Changing the global wind changes
/// the foliage's pose: the meadow's own wind is calm, so all its motion is
/// the global wind's. The frame delta and the projection jitter are fixed,
/// and observations are compared 64 frames apart (the period of the
/// foliage LOD cross-fade dither), so an unchanged frame is identical.
#[test]
fn foliage_sways_in_the_global_wind_on_the_frame_clock() {
    use installed_plugins::foliage_plugin::foliage_component::FoliageComponent;
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

    let mut state = LevelEditorState::new();
    let mut foliage = FoliageComponent::default();
    foliage.general.enabled = true;
    foliage.placement.layer_extent = 20.0;
    // Calm on its own: only the global wind moves it.
    foliage.wind.wind_speed = 0.0;
    foliage.wind.trunk_sway = 1.0;
    execute_command(
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
    assert!(plugin_kinds::create_wind(&mut state).changed);
    set_global_wind(&mut state, |wind| wind.speed = 8.0);

    let mut renderer = harness.renderer(&state);
    renderer.set_frame_delta_override(Some(1.0 / 30.0));
    renderer.set_camera_jitter_override(Some([0.0, 0.0]));
    // Placement fills the camera's ring over several frames.
    let settle = |renderer: &mut HelioRenderer| {
        for _ in 0..5 {
            harness.frames(renderer, || {});
        }
    };
    // Two observations 64 frames apart with no scene write in between: the
    // depth texels that moved.
    let per_observation = settle_frames().max(2) & !1;
    assert_eq!(
        64 % per_observation,
        0,
        "observations must tile the dither period"
    );
    let motion = |renderer: &mut HelioRenderer, label: &str| {
        let first = harness.frames(renderer, || {});
        for _ in 1..64 / per_observation {
            harness.frames(renderer, || {});
        }
        let second = harness.frames(renderer, || {});
        second.dump(&format!("wind_{label}"));
        let moved = second.difference(&first);
        println!("WIND {label}: {moved:?}");
        (moved.depth_texels, second)
    };

    settle(&mut renderer);
    let (blowing, _) = motion(&mut renderer, "blowing");
    assert!(
        blowing > (SIZE * SIZE / 100) as usize,
        "foliage did not sway in the global wind: {blowing} texels moved"
    );

    set_global_wind(&mut state, |wind| wind.speed = 0.0);
    settle(&mut renderer);
    let (calm, _) = motion(&mut renderer, "calm");
    assert_eq!(calm, 0, "foliage moved in a calm");

    set_global_wind(&mut state, |wind| wind.speed = 8.0);
    renderer.set_viewport_realtime(false);
    settle(&mut renderer);
    let (frozen, pose) = motion(&mut renderer, "frozen");
    assert_eq!(frozen, 0, "foliage moved with Realtime off");

    // The clock is frozen, so the pose changes only with the wind.
    set_global_wind(&mut state, |wind| {
        wind.direction = [-1.0, 0.0, -0.2];
        wind.speed = 16.0;
    });
    let changed = harness.frames(&mut renderer, || {});
    let difference = changed.difference(&pose);
    println!("WIND global wind changed: {difference:?}");
    assert!(
        difference.depth_texels > changed.depth.len() / 100,
        "changing the global wind did not change the foliage: {difference:?}"
    );
}

/// Realtime: content that animates on its own (particles,
/// foliage in a wind) keeps a realtime viewport rendering with no scene
/// write; with Realtime off, in a calm, or once the content is disabled,
/// the viewport goes idle again.
#[test]
fn a_realtime_viewport_keeps_rendering_animated_content() {
    use installed_plugins::foliage_plugin::foliage_component::FoliageComponent;
    use installed_plugins::particles_plugin::particles_component::ParticleEmitterComponent;
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
    let mut state = LevelEditorState::new();
    let mut renderer = harness.renderer(&state);
    harness.frames(&mut renderer, || {});

    let view = harness.texture.create_view(&Default::default());
    let encode = |renderer: &mut HelioRenderer| {
        let encoded = renderer
            .render_frame(&harness.device, &harness.queue, &view, SIZE, SIZE, FORMAT)
            .is_some();
        harness
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        encoded
    };
    // Whether the renderer settles to idle within a few frames and then
    // stays idle.
    let goes_idle = |renderer: &mut HelioRenderer| {
        let settled = (0..16).any(|_| !encode(renderer));
        settled && (0..20).all(|_| !encode(renderer))
    };
    let keeps_rendering = |renderer: &mut HelioRenderer| (0..20).all(|_| encode(renderer));
    let add = |state: &mut LevelEditorState, name: &str, component: TypedComponent| {
        let added = execute_command(
            state,
            SceneCommand::AddObjectWithComponents {
                data: SceneObjectData {
                    id: String::new(),
                    name: name.to_string(),
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
        added.affected_ids[0].clone()
    };
    assert!(goes_idle(&mut renderer), "an empty scene did not go idle");

    // Particles.
    let emitter = add(
        &mut state,
        "Particles",
        TypedComponent::new(ParticleEmitterComponent::default()),
    );
    assert!(keeps_rendering(&mut renderer), "live particles went idle");
    renderer.set_viewport_realtime(false);
    assert!(goes_idle(&mut renderer), "frozen particles kept rendering");
    renderer.set_viewport_realtime(true);
    assert!(
        keeps_rendering(&mut renderer),
        "Realtime back on did not resume"
    );
    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &emitter,
        0,
        false
    ));
    assert!(
        goes_idle(&mut renderer),
        "a disabled emitter kept rendering"
    );

    // Water simulates on the frame clock.
    let water = add(
        &mut state,
        "Lake",
        TypedComponent::new(
            installed_plugins::water_plugin::water_component::WaterVolumeComponent::default(),
        ),
    );
    assert!(keeps_rendering(&mut renderer), "live water went idle");
    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &water,
        0,
        false
    ));
    assert!(goes_idle(&mut renderer), "disabled water kept rendering");

    // Foliage in the global wind.
    let mut foliage = FoliageComponent::default();
    foliage.general.enabled = true;
    foliage.wind.wind_speed = 0.0;
    add(&mut state, "meadow", TypedComponent::new(foliage));
    assert!(plugin_kinds::create_wind(&mut state).changed);
    assert!(
        keeps_rendering(&mut renderer),
        "foliage in a wind went idle"
    );
    set_global_wind(&mut state, |wind| wind.speed = 0.0);
    assert!(goes_idle(&mut renderer), "foliage in a calm kept rendering");
    let revision = state.scene.world().revision();
    set_global_wind(&mut state, |wind| wind.speed = 3.0);
    assert!(
        keeps_rendering(&mut renderer),
        "the wind rising did not wake it"
    );
    assert_eq!(
        state.scene.world().revision(),
        revision + 1,
        "animated frames wrote to the world"
    );
}

/// The R channel (height) of one layer of the water simulation's
/// RGBA16F array texture.
fn water_heights(harness: &Harness, renderer: &HelioRenderer, layer: u32) -> Vec<f32> {
    let texture = renderer
        .debug_pass::<helio_pass_water_sim::WaterSimPass>()
        .map(|pass| pass.sim_texture())
        .expect("the editor graph simulates water");
    let size = texture.size();
    let row = size.width * 8;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = harness.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("water-sim-readback"),
        size: (padded * size.height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = harness.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
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
    harness.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    harness
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let mapped = slice.get_mapped_range().expect("mapped range");
    let heights = mapped
        .chunks_exact(padded as usize)
        .flat_map(|line| {
            line[..row as usize]
                .chunks_exact(8)
                .map(|texel| f16_to_f32(u16::from_le_bytes([texel[0], texel[1]])))
                .collect::<Vec<_>>()
        })
        .collect();
    drop(mapped);
    buffer.unmap();
    heights
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x03ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        0x1f => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// Mean absolute height: how far a layer is from flat water.
fn roughness(heights: &[f32]) -> f32 {
    heights.iter().map(|h| h.abs()).sum::<f32>() / heights.len() as f32
}

/// Water dynamics: each volume simulates on its own layers, read
/// back from the simulation: a body pushing the lake moves the lake while
/// the calm pond beside it stays flat, a body in the pond moves the pond,
/// and the simulation advances on the frame clock, so it stands still with
/// Realtime off. Wind seas come from the spectral ocean, not this solver
/// (OPTICS.md: the volume solver carries local disturbances), so a volume's
/// own wind shows on its surface, not in these layers.
#[test]
fn water_volumes_simulate_their_own_disturbances_on_the_frame_clock() {
    use installed_plugins::physics_plugin::legacy::{CollisionChannel, PhysicsComponent};
    use installed_plugins::water_plugin::water_component::WaterVolumeComponent;
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
    let mut state = LevelEditorState::new();
    let water = WaterVolumeComponent {
        size: [20.0, 4.0, 20.0],
        use_global_wind: false,
        wind_strength: 0.0,
        ..Default::default()
    };
    let add = |state: &mut LevelEditorState, name: &str, x: f32| {
        execute_command(
            state,
            SceneCommand::AddObjectWithComponents {
                data: SceneObjectData {
                    id: String::new(),
                    name: name.to_string(),
                    object_type: ObjectType::Empty,
                    transform: Transform {
                        position: [f64::from(x), 0.0, 0.0],
                        ..Transform::default()
                    },
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
                components: vec![TypedComponent::new(water.clone())],
            },
        )
        .affected_ids[0]
            .clone()
    };
    // A 1 m cube (SM_Cube is 200 units across) half under the surface, on
    // the WaterSim collision channel.
    let body = |state: &mut LevelEditorState, position: [f32; 3]| {
        let id = drop_matte_mesh(state);
        let placed = execute_command(
            state,
            SceneCommand::SetTransform {
                id: id.clone(),
                position: Some(position.map(f64::from)),
                rotation: None,
                scale: Some([0.005; 3]),
            },
        );
        assert!(placed.changed);
        let mut physics = PhysicsComponent::default();
        physics.collision.collision_channel |= u64::from(CollisionChannel::WaterSim);
        components::add_component_value(
            &mut state.scene.world_mut(),
            &id,
            "PhysicsComponent",
            Some(Box::new(physics)),
        )
        .expect("physics attached");
        id
    };
    // The lake takes the first water row (layers 0..3), the pond the
    // second (layers 3..6).
    add(&mut state, "lake", 0.0);
    add(&mut state, "pond", 100.0);
    let swimmer = body(&mut state, [2.0, 0.0, 2.0]);
    let mut renderer = harness.renderer(&state);
    renderer.set_frame_delta_override(Some(1.0 / 60.0));
    harness.frames(&mut renderer, || {});
    harness.frames(&mut renderer, || {});

    let lake_waves = roughness(&water_heights(&harness, &renderer, 0));
    let pond_waves = roughness(&water_heights(&harness, &renderer, 3));
    println!("WATER lake {lake_waves} pond {pond_waves}");
    assert!(lake_waves > 1e-4, "the pushed lake is flat: {lake_waves}");
    assert_eq!(pond_waves, 0.0, "the pond moved with the lake");

    // The body swims over to the pond: the pond's own layers move.
    move_to(&mut state, &swimmer, [102.0, 0.0, 2.0]);
    harness.frames(&mut renderer, || {});
    harness.frames(&mut renderer, || {});
    let pond_waves = roughness(&water_heights(&harness, &renderer, 3));
    println!("WATER pond pushed {pond_waves}");
    assert!(
        pond_waves > 1e-4,
        "the body did not move the pond: {pond_waves}"
    );

    // Realtime off freezes the frame clock, and the water with it.
    renderer.set_viewport_realtime(false);
    harness.frames(&mut renderer, || {});
    let before = water_heights(&harness, &renderer, 0);
    harness.frames(&mut renderer, || {});
    let after = water_heights(&harness, &renderer, 0);
    assert!(before == after, "the water moved with Realtime off");
    renderer.set_viewport_realtime(true);
    harness.frames(&mut renderer, || {});
    assert!(
        water_heights(&harness, &renderer, 0) != after,
        "Realtime back on did not resume the water"
    );
}

/// Water interaction: a body on the `WaterSim` collision channel
/// pushes the water it sits in, read back from the simulation: the water
/// under it is displaced while the water under an identical body off the
/// channel is not; a body at rest does not keep pushing (the water's mass
/// stays put); moving it displaces the water where it goes.
#[test]
fn bodies_on_the_water_sim_channel_push_the_water() {
    use installed_plugins::physics_plugin::legacy::{CollisionChannel, PhysicsComponent};
    use installed_plugins::water_plugin::water_component::WaterVolumeComponent;
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
    let mut state = LevelEditorState::new();
    // A calm pool at the origin, its surface at Y 0.
    execute_command(
        &mut state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: "pool".to_string(),
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
            components: vec![TypedComponent::new(WaterVolumeComponent {
                size: [40.0, 8.0, 40.0],
                wind_strength: 0.0,
                ..Default::default()
            })],
        },
    );
    // Two 1 m cubes (SM_Cube is 200 units across) half under the surface,
    // 15 m apart: half of cascade 0's 30 m tile, so their texels are as far
    // apart as the tile allows.
    let body = |state: &mut LevelEditorState, position: [f32; 3], water: bool| {
        let id = drop_matte_mesh(state);
        let placed = execute_command(
            state,
            SceneCommand::SetTransform {
                id: id.clone(),
                position: Some(position.map(f64::from)),
                rotation: None,
                scale: Some([0.005; 3]),
            },
        );
        assert!(placed.changed);
        let mut physics = PhysicsComponent::default();
        if water {
            physics.collision.collision_channel |= u64::from(CollisionChannel::WaterSim);
        }
        components::add_component_value(
            &mut state.scene.world_mut(),
            &id,
            "PhysicsComponent",
            Some(Box::new(physics)),
        )
        .expect("physics attached");
        id
    };
    let swimmer = body(&mut state, [-7.5, 0.0, 15.0], true);
    body(&mut state, [7.5, 0.0, 15.0], false);

    // Cascade 0 texel of world XZ: uv = fract(xz / 30).
    let texel = |x: f32, z: f32| {
        let u = (x / 30.0).rem_euclid(1.0);
        let v = (z / 30.0).rem_euclid(1.0);
        ((u * 256.0) as usize, (v * 256.0) as usize)
    };
    // Mean |height| within 6 texels of a point, and the layer's total.
    let around = |heights: &[f32], (cx, cy): (usize, usize)| {
        let mut sum = 0.0;
        let mut n = 0;
        for y in cy.saturating_sub(6)..(cy + 7).min(256) {
            for x in cx.saturating_sub(6)..(cx + 7).min(256) {
                sum += heights[y * 256 + x].abs();
                n += 1;
            }
        }
        sum / n as f32
    };
    let mass = |heights: &[f32]| heights.iter().sum::<f32>();

    let mut renderer = harness.renderer(&state);
    renderer.set_frame_delta_override(Some(1.0 / 60.0));
    harness.frames(&mut renderer, || {});
    let heights = water_heights(&harness, &renderer, 0);
    let pushed = around(&heights, texel(-7.5, 15.0));
    let untouched = around(&heights, texel(7.5, 15.0));
    let displaced = mass(&heights);
    println!("HITBOX pushed {pushed} untouched {untouched} mass {displaced}");
    assert!(
        pushed > 1e-3,
        "the body on the WaterSim channel did not push the water: {pushed}"
    );
    assert_eq!(untouched, 0.0, "the body off the channel pushed the water");
    assert!(
        displaced < 0.0,
        "the body did not displace water: {displaced}"
    );

    // At rest it pushes no further: the water's mass stays where it was.
    harness.frames(&mut renderer, || {});
    let resting = mass(&water_heights(&harness, &renderer, 0));
    println!("HITBOX resting mass {resting}");
    assert!(
        (resting - displaced).abs() < displaced.abs() * 0.25,
        "a body at rest kept displacing water: {displaced} then {resting}"
    );

    // Moving it pushes the water where it goes and lets it back where it was.
    let before = water_heights(&harness, &renderer, 0);
    move_to(&mut state, &swimmer, [-7.5, 0.0, 7.5]);
    harness.frames(&mut renderer, || {});
    let after = water_heights(&harness, &renderer, 0);
    let at = |heights: &[f32], (x, y): (usize, usize)| heights[y * 256 + x];
    let (new_spot, old_spot) = (texel(-7.5, 7.5), texel(-7.5, 15.0));
    println!(
        "HITBOX moved: new spot {} -> {}, old spot {} -> {}",
        at(&before, new_spot),
        at(&after, new_spot),
        at(&before, old_spot),
        at(&after, old_spot)
    );
    assert!(
        at(&after, new_spot) < at(&before, new_spot) - 1e-3,
        "the water did not fall where the body moved to"
    );
    assert!(
        at(&after, old_spot) > at(&before, old_spot) + 1e-3,
        "the water did not rise where the body left"
    );
}

/// The final image's RGBA at pixel `(x, y)`.
fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    frame.color[i..i + 4].try_into().unwrap()
}

#[track_caller]
fn assert_color(what: &str, actual: [u8; 4], expected: [u8; 3]) {
    let close = (0..3).all(|i| actual[i].abs_diff(expected[i]) <= 8);
    assert!(close, "{what}: {actual:?}, expected {expected:?}");
}

/// 2D sprites: an authored sprite draws at its 2D position (pixels
/// from the centre of the view, Y up) on top of the 3D scene; between two
/// overlapping sprites the higher Z index is on top, and changing it
/// changes which; a textured sprite shows its image; moving the 3D camera
/// does not move a sprite, moving its owner does, and disabling sprites
/// restores the 3D frame.
#[test]
fn sprites_draw_in_2d_over_the_3d_scene() {
    use installed_plugins::sprites_plugin::sprites_component::SpriteComponent;
    use plugin_kinds::create_sprite;

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
    let mut state = LevelEditorState::new();
    drop_mesh(&mut state);
    let mut renderer = harness.renderer(&state);
    let scene = harness.frames(&mut renderer, || {});
    scene.dump("sprites_scene");
    let centre = SIZE / 2;
    assert!(
        pixel(&scene, centre, centre)[..3] != [255, 0, 0],
        "the mesh under the sprite is not red"
    );

    let square = |tint: [f32; 4], z_index: i32| SpriteComponent {
        tint,
        width: 40.0,
        height: 40.0,
        z_index,
        ..Default::default()
    };
    let red = create_sprite(&mut state, [0.0, 0.0], square([1.0, 0.0, 0.0, 1.0], 0)).affected_ids
        [0]
    .clone();
    let green = create_sprite(&mut state, [15.0, 0.0], square([0.0, 1.0, 0.0, 1.0], 1))
        .affected_ids[0]
        .clone();
    let frame = harness.frames(&mut renderer, || {});
    frame.dump("sprites_two");
    assert_color(
        "the red sprite over the mesh",
        pixel(&frame, centre - 10, centre),
        [255, 0, 0],
    );
    assert_color(
        "the overlap (green above red)",
        pixel(&frame, centre + 8, centre),
        [0, 255, 0],
    );
    assert_color(
        "the green sprite alone",
        pixel(&frame, centre + 30, centre),
        [0, 255, 0],
    );
    // Outside both sprites the 3D frame is untouched.
    for (x, y) in [
        (centre - 40, centre),
        (centre, centre - 40),
        (centre + 50, centre + 30),
    ] {
        assert_eq!(
            pixel(&frame, x, y),
            pixel(&scene, x, y),
            "beside the sprites at {x},{y}"
        );
    }

    // The Z index orders them.
    let red_sprite = first_instance(&state, &red);
    state
        .scene
        .world_mut()
        .get_mut::<SpriteComponent>(red_sprite)
        .unwrap()
        .z_index = 2;
    let frame = harness.frames(&mut renderer, || {});
    assert_color(
        "the overlap (red above green)",
        pixel(&frame, centre + 8, centre),
        [255, 0, 0],
    );

    // The 3D camera moves the mesh, not the sprites.
    renderer.set_editor_camera_state(EditorCameraState {
        yaw: harness.camera.yaw + 0.4,
        ..harness.camera
    });
    let turned = harness.frames(&mut renderer, || {});
    turned.dump("sprites_turned");
    assert!(
        turned.difference(&frame).depth_texels > 0,
        "the camera move did not move the 3D scene"
    );
    for (x, y) in [
        (centre - 10, centre),
        (centre + 8, centre),
        (centre + 30, centre),
    ] {
        assert_eq!(
            pixel(&turned, x, y),
            pixel(&frame, x, y),
            "a sprite moved with the camera at {x},{y}"
        );
    }
    renderer.set_editor_camera_state(harness.camera);

    // Its owner places it: up and to the left is up and to the left.
    move_to(&mut state, &red, [-60.0, 30.0, 0.0]);
    let frame = harness.frames(&mut renderer, || {});
    assert_color(
        "the moved red sprite",
        pixel(&frame, centre - 60, centre - 30),
        [255, 0, 0],
    );
    assert_color(
        "where the red sprite was",
        pixel(&frame, centre, centre),
        [0, 255, 0],
    );

    // An image: a 2x1 sheet, blue then yellow.
    let dir = std::env::temp_dir().join(format!("pulsar-sprite-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("blue_yellow.png");
    image::save_buffer(
        &image,
        &[0, 0, 255, 255, 255, 255, 0, 255],
        2,
        1,
        image::ColorType::Rgba8,
    )
    .unwrap();
    let textured = create_sprite(
        &mut state,
        [60.0, -60.0],
        SpriteComponent {
            texture: image.to_string_lossy().into_owned().into(),
            ..square([1.0, 1.0, 1.0, 1.0], 0)
        },
    )
    .affected_ids[0]
        .clone();
    // The world loads the image off the write path (SceneDB#84).
    assert!(state
        .scene
        .world_mut()
        .resolve_assets_blocking(std::time::Duration::from_secs(30)));
    let frame = harness.frames(&mut renderer, || {});
    frame.dump("sprites_textured");
    assert_color(
        "the image's left half",
        pixel(&frame, centre + 50, centre + 60),
        [0, 0, 255],
    );
    assert_color(
        "the image's right half",
        pixel(&frame, centre + 70, centre + 60),
        [255, 255, 0],
    );

    // The file changes: the sprite shows the new image, re-uploaded.
    image::save_buffer(
        &image,
        &[255, 255, 0, 255, 0, 0, 255, 255],
        2,
        1,
        image::ColorType::Rgba8,
    )
    .unwrap();
    {
        let mut world = state.scene.world_mut();
        assert_eq!(
            world.asset_source_changed(&helio_component::texture_assets::texture_source_for_file(
                &image
            )),
            1
        );
        assert!(world.resolve_assets_blocking(std::time::Duration::from_secs(30)));
    }
    let frame = harness.frames(&mut renderer, || {});
    assert_color(
        "the reloaded image's left half",
        pixel(&frame, centre + 50, centre + 60),
        [255, 255, 0],
    );
    assert_color(
        "the reloaded image's right half",
        pixel(&frame, centre + 70, centre + 60),
        [0, 0, 255],
    );

    // Disabled, the 3D frame is back.
    for id in [&red, &green, &textured] {
        let instance = first_instance(&state, id);
        state
            .scene
            .world_mut()
            .get_mut::<SpriteComponent>(instance)
            .unwrap()
            .enabled = false;
    }
    let frame = harness.frames(&mut renderer, || {});
    for (x, y) in [
        (centre - 60, centre - 30),
        (centre - 10, centre),
        (centre + 8, centre),
        (centre + 50, centre + 60),
    ] {
        assert_eq!(
            pixel(&frame, x, y),
            pixel(&scene, x, y),
            "a disabled sprite at {x},{y}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An authored camera lens reaches the lens pass (the host registers its
/// buffers, the component derives the rows): a mesh far from the focus
/// distance blurs, and disabling the lens restores the frame.
#[test]
fn a_camera_lens_defocuses_the_frame_and_follows_its_enabled_state() {
    use installed_plugins::lens_plugin::lens_component::CameraLensComponent;
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

    // Focused a few centimetres from the lens, wide open: the mesh is far
    // outside the depth of field.
    let mut lens = CameraLensComponent::default();
    lens.view_id = 0;
    lens.focal_length_mm = 85.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 0.3;
    lens.max_coc_px = 32.0;
    let mut state = LevelEditorState::new();
    drop_mesh(&mut state);
    let mut renderer = harness.renderer(&state);
    let before = harness.frames(&mut renderer, || {});
    let added = execute_command(
        &mut state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: "lens".to_string(),
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
            components: vec![TypedComponent::new(lens)],
        },
    );
    let id = added.affected_ids[0].clone();
    let with = harness.frames(&mut renderer, || {});
    with.dump("camera_lens_defocus");
    let change = with.difference(&before);
    println!("LENS defocus vs without: {change:?}");
    assert!(
        change.color_pixels > (SIZE * SIZE / 100) as usize,
        "the lens did not change the frame: {change:?}"
    );
    assert_eq!(change.depth_texels, 0, "a lens moved geometry");

    assert!(components::set_component_enabled(
        &mut state.scene.world_mut(),
        &id,
        0,
        false
    ));
    let disabled = harness.frames(&mut renderer, || {});
    let change = disabled.difference(&before);
    println!("LENS disabled vs without: {change:?}");
    assert!(
        change.color_pixels < (SIZE * SIZE / 100) as usize,
        "a disabled lens still changes the frame: {change:?}"
    );
}

/// A lens that maps past the frame (an equidistant fisheye) makes the viewport
/// camera render a wider view, which the lens pass crops back to the frame:
/// the scene's depth buffer is that of a wider camera while a lens that stays
/// inside the frame (the defocus test above) leaves it untouched.
#[test]
fn a_fisheye_lens_widens_the_render_and_the_lens_crops_it_back() {
    use installed_plugins::lens_plugin::lens_component::{
        CameraLensComponent, LensElementKind, LensElementProps, LensProjectionModel,
        LensRegionShape,
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

    // In focus at the mesh, so the lens changes the frame by its geometry only.
    let mut lens = CameraLensComponent::default();
    lens.view_id = 0;
    lens.focal_length_mm = 35.0;
    lens.f_number = 16.0;
    lens.focus_distance_m = 10.0;
    lens.elements = vec![LensElementProps {
        kind: LensElementKind::Projection,
        shape: LensRegionShape::FullFrame,
        projection_model: LensProjectionModel::Equidistant,
        ..LensElementProps::default()
    }];
    let mut state = LevelEditorState::new();
    drop_mesh(&mut state);
    let mut renderer = harness.renderer(&state);
    let before = harness.frames(&mut renderer, || {});
    execute_command(
        &mut state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: "lens".to_string(),
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
            components: vec![TypedComponent::new(lens)],
        },
    );
    let with = harness.frames(&mut renderer, || {});
    with.dump("camera_lens_fisheye_overscan");
    let change = with.difference(&before);
    println!("LENS fisheye vs without: {change:?}");
    assert!(
        change.color_pixels > (SIZE * SIZE / 100) as usize,
        "the fisheye did not change the frame: {change:?}"
    );
    assert!(
        change.depth_texels > 0,
        "the camera did not render the wider view the fisheye reaches into: {change:?}"
    );
}

/// The lens elements' registry entries come with the editor graph: the water
/// drops of the "Rainy lens" preset read the bundled mask array and a
/// `Prescription` element reads the bundled bank, so each changes the frame
/// against the same lens with its effect switched off.
#[test]
fn the_bundled_lens_masks_and_prescriptions_reach_the_lens_pass() {
    use installed_plugins::lens_plugin::lens_component::{
        CameraLensComponent, LensElementKind, LensElementProps, LensPrescriptionChoice, LensPreset,
        LensRegionShape,
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

    // Renders the mesh through `lens`, in focus so only the lens' own
    // elements change the frame. A bright wall behind it fills the frame to
    // its corners, where a prescription's falloff shows.
    let render = |lens: CameraLensComponent| {
        let mut state = LevelEditorState::new();
        drop_mesh(&mut state);
        let wall = drop_mesh(&mut state);
        move_to(&mut state, &wall, [0.0, 0.0, -radius * 3.0]);
        transform(&mut state, &wall, None, Some([12.0, 12.0, 0.05]));
        let mut renderer = harness.renderer(&state);
        execute_command(
            &mut state,
            SceneCommand::AddObjectWithComponents {
                data: SceneObjectData {
                    id: String::new(),
                    name: "lens".to_string(),
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
                components: vec![TypedComponent::new(lens)],
            },
        );
        harness.frames(&mut renderer, || {})
    };
    let focused = |mut lens: CameraLensComponent| {
        lens.f_number = 16.0;
        lens.focus_distance_m = 10.0;
        lens
    };

    // Water drops: the same lens with the refraction at zero is the control.
    let mut drops = CameraLensComponent::default();
    drops.preset = LensPreset::RainyLens;
    let mut dry = drops.resolved();
    dry.preset = LensPreset::Custom;
    for element in &mut dry.elements {
        if element.kind == LensElementKind::WaterDrops {
            element.drops_refraction = 0.0;
        }
    }
    let wet = render(focused(drops.resolved()));
    let dry = render(focused(dry));
    let change = wet.difference(&dry);
    println!("LENS rainy vs dry: {change:?}");
    assert!(
        change.color_pixels > 0,
        "the water drops did nothing: no bundled mask array: {change:?}"
    );

    // A prescription: the ideal thin lens is the identity map with the bake's
    // natural falloff, so its corners darken against the same lens without it.
    let mut baked = CameraLensComponent::default();
    baked.elements = vec![LensElementProps {
        kind: LensElementKind::Prescription,
        shape: LensRegionShape::FullFrame,
        prescription_lens: LensPrescriptionChoice::ThinLens50F2,
        ..LensElementProps::default()
    }];
    let mut bare = baked.clone();
    bare.elements.clear();
    let bare = render(focused(bare));
    // The bundled bank is traced on a background thread the lens starts
    // (`start_baking`; minutes in a debug build): wait for it to arrive.
    let start = std::time::Instant::now();
    let change = loop {
        let change = render(focused(baked.clone())).difference(&bare);
        if change.color_pixels > 0 || start.elapsed() > std::time::Duration::from_secs(600) {
            break change;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    println!("LENS thin-lens prescription vs none: {change:?}");
    assert!(
        change.color_pixels > 0,
        "the prescription did nothing: no bundled bank: {change:?}"
    );
    assert_eq!(change.depth_texels, 0, "an identity map moved geometry");
}

/// The toolbar's Depth and Normals toggles reach the graph through the
/// editor mailbox: the viewport shows the raw scene depth (grey, nothing over
/// the sky) or the world normals instead of the lit image, and switching back
/// restores the lit frame.
#[test]
fn the_viewport_shows_the_depth_and_normal_buffers() {
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
    let mut state = LevelEditorState::new();
    drop_mesh(&mut state);
    let mut renderer = harness.renderer(&state);
    let mailbox = renderer.editor_mailbox();
    let lit = harness.frames(&mut renderer, || {});

    mailbox.set_viewport_debug_mode(helio_pass_buffer_view::DEPTH_VIEW);
    let depth = harness.frames(&mut renderer, || {});
    depth.dump("viewport_depth_view");
    assert!(
        depth.difference(&lit).color_pixels > (SIZE * SIZE / 10) as usize,
        "the depth view did not replace the lit image"
    );
    assert!(
        depth
            .color
            .chunks_exact(4)
            .all(|p| p[0] == p[1] && p[1] == p[2]),
        "the depth view is not greyscale"
    );
    let lit_texels = depth.color.chunks_exact(4).filter(|p| p[0] > 0).count();
    assert!(
        lit_texels > (SIZE * SIZE / 100) as usize,
        "the mesh shows no depth"
    );

    mailbox.set_viewport_debug_mode(helio_pass_buffer_view::NORMALS_VIEW);
    let normals = harness.frames(&mut renderer, || {});
    normals.dump("viewport_normals_view");
    assert!(normals.difference(&depth).color_pixels > (SIZE * SIZE / 100) as usize);
    assert!(
        normals
            .color
            .chunks_exact(4)
            .any(|p| p[0] != p[1] || p[1] != p[2]),
        "the normals view is greyscale"
    );

    mailbox.set_viewport_debug_mode(0);
    let back = harness.frames(&mut renderer, || {});
    let change = back.difference(&lit);
    assert!(
        change.color_pixels < (SIZE * SIZE / 100) as usize,
        "switching back did not restore the lit image: {change:?}"
    );
}

/// A mirrored mesh (a negative-determinant scale) shows its outside: the
/// cube is symmetric about its pivot, so mirroring it in X must leave the
/// frame's depth and the viewport's normals view as they were. A pass that
/// keeps culling back faces by the unmirrored winding shows the cube's
/// inside instead (farther depth, normals facing away).
#[test]
fn a_mirrored_mesh_shows_its_outside() {
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
    let mut state = LevelEditorState::new();
    let id = drop_mesh(&mut state);
    // Turned so three faces show, each with its own normal.
    transform(&mut state, &id, Some([20.0, 35.0, 0.0]), None);
    let mut renderer = harness.renderer(&state);
    let mailbox = renderer.editor_mailbox();
    mailbox.set_viewport_debug_mode(helio_pass_buffer_view::NORMALS_VIEW);
    let plain = harness.frames(&mut renderer, || {});
    plain.dump("mirror_plain_normals");

    for (what, scale) in [
        ("mirrored in X", [-1.0, 1.0, 1.0]),
        ("mirrored in Y and Z (a rotation)", [1.0, -1.0, -1.0]),
        ("mirrored in X, Y and Z", [-1.0, -1.0, -1.0]),
    ] {
        transform(&mut state, &id, None, Some(scale));
        let mirrored = harness.frames(&mut renderer, || {});
        mirrored.dump(&format!("mirror_{}_normals", what.replace(' ', "_")));
        mirrored.dump_depth_change(&plain, &format!("mirror_{}_depth", what.replace(' ', "_")));
        let change = mirrored.difference(&plain);
        println!("{what}: {change:?}");
        assert!(
            change.depth_texels < (SIZE * SIZE / 200) as usize
                && change.color_pixels < (SIZE * SIZE / 200) as usize,
            "{what}: the cube does not show its outside: {change:?}"
        );
    }
}

/// A mesh put in a folder moves with the folder, in the rendered frame.
#[test]
fn a_folders_meshes_move_with_it_in_the_frame() {
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

    for nudge in [false, true] {
        harness.nudge_camera.set(nudge);
        let mut state = LevelEditorState::new();
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {});
        let mesh = drop_mesh(&mut state);
        let folder = add_object(&mut state, "folder", ObjectType::Folder);
        let reparent = execute_command(
            &mut state,
            SceneCommand::ReparentObject {
                id: mesh.clone(),
                new_parent_id: Some(folder.clone()),
            },
        );
        assert!(reparent.changed, "the mesh was not put in the folder");
        let before = harness.frames(&mut renderer, || {});
        let at_origin = Frame {
            color: before.color.clone(),
            depth: before.depth.clone(),
            depth_width: before.depth_width,
        };
        assert!(
            before.difference(&at_origin).depth_texels == 0,
            "sanity: a frame equals itself"
        );

        move_to(&mut state, &folder, [radius * 1.5, 0.0, 0.0]);
        let after = harness.frames(&mut renderer, || {});
        let shift = after.difference(&at_origin);
        assert!(
            shift.depth_texels > 0,
            "[nudge {nudge}] moving the folder did not move the mesh inside it ({shift:?})"
        );
    }
}

/// A mesh inside a rigid frame is drawn through the coordinate-space table:
/// moving the frame moves it in the rendered frame, though its own row never
/// changes, and turning the frame back into an ordinary folder keeps it there.
#[test]
fn a_rigid_frames_meshes_move_with_it_in_the_frame() {
    use engine_backend::scene::frames::set_rigid_frame;
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

    for nudge in [false, true] {
        harness.nudge_camera.set(nudge);
        let mut state = LevelEditorState::new();
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {});
        let mesh = drop_mesh(&mut state);
        let folder = add_object(&mut state, "ship", ObjectType::Folder);
        let reparent = execute_command(
            &mut state,
            SceneCommand::ReparentObject {
                id: mesh.clone(),
                new_parent_id: Some(folder.clone()),
            },
        );
        assert!(reparent.changed, "the mesh was not put in the folder");
        let entity = state.scene.world().entity_for(&folder).expect("the ship");
        set_rigid_frame(&mut state.scene.world_mut(), entity, true).expect("a free frame slot");
        let before = harness.frames(&mut renderer, || {});
        assert!(
            before.depth.iter().zip(&before.depth).all(|(a, b)| a == b),
            "sanity"
        );
        // The mesh is drawn (not lost to a wrong space): compare with an empty scene.
        let empty = {
            let state = LevelEditorState::new();
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {})
        };
        assert_drawn(
            &format!("[nudge {nudge}] a mesh inside a rigid frame"),
            before.difference(&empty),
        );
        let at_origin = Frame {
            color: before.color.clone(),
            depth: before.depth.clone(),
            depth_width: before.depth_width,
        };

        move_to(&mut state, &folder, [radius * 1.5, 0.0, 0.0]);
        let after = harness.frames(&mut renderer, || {});
        let shift = after.difference(&at_origin);
        assert!(
            shift.depth_texels > 0,
            "[nudge {nudge}] moving the frame did not move the mesh inside it ({shift:?})"
        );
        assert_drawn(
            &format!("[nudge {nudge}] the moved frame's mesh"),
            after.difference(&empty),
        );
    }
}

/// A mesh on each side of the far portal (see [`portal_scene`]).
fn add_portal_content(state: &mut LevelEditorState, radius: f32) {
    for z in [radius * 2.0, -radius * 2.0] {
        let mesh = drop_mesh(state);
        move_to(state, &mesh, [100.0, 0.0, z]);
    }
}

/// Two portals (Link IDs `near_id` and `far_id`), the near one at the origin facing
/// the test camera and the far one 100 m to the side, with a mesh on each side of the
/// far one. The meshes are out of the camera's view unless the portals are linked.
fn portal_scene(radius: f32, near_id: i32, far_id: i32, content: bool) -> LevelEditorState {
    portal_scene_at(radius, near_id, far_id, content, 100.0)
}

/// A portal object named `name` with Link ID `link`, `radius * 3` across.
fn add_portal(
    state: &mut LevelEditorState,
    radius: f32,
    name: &str,
    position: [f32; 3],
    link: i32,
) -> String {
    let id = add_object(state, name, ObjectType::Empty);
    if position != [0.0; 3] {
        move_to(state, &id, position);
    }
    assert!(
        execute_command(
            state,
            SceneCommand::AddComponent {
                id: id.clone(),
                class_name: "PortalComponent".into(),
                value: None,
            },
        )
        .changed,
        "the portal component was refused"
    );
    for (property, value) in [
        ("link_id", Box::new(link) as Box<dyn std::any::Any + Send>),
        ("width", Box::new(radius * 3.0)),
        ("height", Box::new(radius * 3.0)),
    ] {
        let result = execute_command(
            state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "PortalComponent".into(),
                component_index: 0,
                prop_name: property.into(),
                value,
            },
        );
        assert!(result.changed, "portal {property} edit was refused");
    }
    id
}

/// [`portal_scene`] with the far portal `far_x` metres to the side.
fn portal_scene_at(
    radius: f32,
    near_id: i32,
    far_id: i32,
    content: bool,
    far_x: f32,
) -> LevelEditorState {
    let mut state = LevelEditorState::new();
    // An opening shows its far side to eyes on its local -Z side: turned, the
    // near one faces the camera (on +Z).
    let near = add_portal(&mut state, radius, "near", [0.0, 0.0, 0.0], near_id);
    rotate_y(&mut state, &near, 180.0);
    add_portal(&mut state, radius, "far", [far_x, 0.0, 0.0], far_id);
    if content {
        add_portal_content(&mut state, radius);
    }
    state
}

#[test]
fn linked_portal_keeps_showing_content_inside_the_near_plane() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    let render = |far_id| {
        let mut renderer = harness.renderer(&portal_scene(radius, 5, far_id, true));
        renderer.set_editor_camera_state(EditorCameraState {
            position: [0.0, 0.0, 0.001],
            yaw: 0.0,
            pitch: 0.0,
        });
        harness.frames(&mut renderer, || {})
    };
    let shown = render(5).difference(&render(2));
    assert!(
        shown.depth_texels > 0,
        "destination geometry disappeared inside the near plane: {shown:?}"
    );
}

#[test]
fn editor_camera_follows_a_link_across_rigid_frames_when_enabled() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    let far_x = 5000.0;
    let mut state = LevelEditorState::new();
    let near = add_portal(&mut state, 1.0, "near", [0.0; 3], 5);
    rotate_y(&mut state, &near, 180.0);
    let far = add_portal(&mut state, 1.0, "far", [far_x, 0.0, 0.0], 5);
    let room = add_object(&mut state, "destination", ObjectType::Folder);
    let parent = state.scene.world().entity_for(&room).unwrap();
    engine_backend::scene::frames::set_rigid_frame(&mut state.scene.world_mut(), parent, true)
        .unwrap();
    assert!(
        execute_command(
            &mut state,
            SceneCommand::ReparentObject {
                id: far,
                new_parent_id: Some(room)
            },
        )
        .changed
    );

    let mut renderer = harness.renderer(&state);
    // The eye starts above the opening. Level flight misses while flight
    // along the pitched view direction passes through its centre.
    let start = EditorCameraState {
        position: [0.0, 2.0, 2.0],
        yaw: 0.0,
        pitch: -std::f32::consts::FRAC_PI_4,
    };
    for follow in [false, true] {
        renderer.set_editor_camera_state(start);
        renderer
            .editor_mailbox()
            .set_follow_space_transitions(follow);
        harness.frames(&mut renderer, || {});
        {
            let mut input = renderer.camera_input.lock().unwrap();
            input.forward = 1.0;
            input.move_speed = 1000.0;
        }
        harness.frames(&mut renderer, || {});
        renderer.camera_input.lock().unwrap().forward = 0.0;
        let [x, _, z] = renderer.editor_camera_state().position;
        assert!(z < 0.0, "camera did not cross the aperture plane");
        let expected_x = if follow { f64::from(far_x) } else { 0.0 };
        assert!((x - expected_x).abs() < 1e-3, "follow={follow}, eye x={x}");
    }
}

/// Mean `[r, g, b]` (0-255) over the pixels where `frame` differs from `reference`.
fn mean_color_of_difference(frame: &Frame, reference: &Frame) -> Option<[f32; 3]> {
    let mut sum = [0.0f64; 3];
    let mut count = 0u64;
    for (a, b) in frame
        .color
        .chunks_exact(4)
        .zip(reference.color.chunks_exact(4))
    {
        if (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > PIXEL_THRESHOLD {
            for i in 0..3 {
                sum[i] += f64::from(a[i]);
            }
            count += 1;
        }
    }
    (count > 0).then(|| sum.map(|v| (v / count as f64) as f32))
}

/// Two linked portals show what is beside the far one, through the near one.
/// The same scene with mismatched Link IDs shows nothing of it: the meshes sit
/// 100 m to the side, out of the camera's view.
#[test]
fn linked_portals_show_the_far_side_in_the_frame() {
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
    let render = |state: &LevelEditorState| {
        let mut renderer = harness.renderer(state);
        harness.frames(&mut renderer, || {})
    };
    let unlinked = render(&portal_scene(radius, 1, 2, true));
    let linked = render(&portal_scene(radius, 5, 5, true));
    linked.dump("portal_linked");
    unlinked.dump("portal_unlinked");
    linked.dump_depth_change(&unlinked, "portal_linked_depth");
    let shown = linked.difference(&unlinked);
    println!("PORTAL linked vs unlinked: {shown:?}");
    assert!(
        shown.depth_texels > 0,
        "a linked portal showed nothing of the content beside its partner ({shown:?})"
    );
}

/// Portals cost nothing where a level has none: the chains, the mask and
/// every drawer's draw through portals are scheduled only while the level
/// has linked portals (the frame publishes chains), and all of them are then.
#[test]
fn portal_passes_are_scheduled_only_with_linked_portals() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    const THROUGH_PORTALS: [&str; 5] = [
        "PortalChains",
        "PortalMask",
        "PortalOccurrences",
        "GBufferPortalOccurrences",
        "FoliageGBufferPortalOccurrences",
    ];
    let members = |state: &LevelEditorState| {
        let mut renderer = harness.renderer(state);
        harness.frames(&mut renderer, || {});
        renderer.graph_members()
    };
    for (label, state) in [
        ("no portals", {
            let mut state = LevelEditorState::new();
            drop_matte_mesh(&mut state);
            state
        }),
        ("unlinked portals", portal_scene(radius, 1, 2, true)),
    ] {
        let scheduled = members(&state);
        assert!(scheduled.contains(&"GBuffer"), "{label}: {scheduled:?}");
        for pass in THROUGH_PORTALS {
            assert!(
                !scheduled.contains(&pass),
                "{label}: {pass} is scheduled: {scheduled:?}"
            );
        }
    }
    let scheduled = members(&portal_scene(radius, 5, 5, true));
    for pass in THROUGH_PORTALS {
        assert!(
            scheduled.contains(&pass),
            "linked portals: {pass} is not scheduled: {scheduled:?}"
        );
    }
}

/// What a portal shows moves with the camera under TSR: its motion vectors
/// carry the chain's map (this frame's and last frame's), so content seen
/// through an opening reprojects like the same content seen directly. The
/// camera strafes and approaches a step a frame; where the content shows, the
/// last moving frame is compared with the frame settled at the same camera,
/// through the portal and, as the control, with the content placed directly
/// where the portal shows it (a smear through the portal would trail it
/// beyond what TSR lags behind direct motion).
#[test]
fn portal_content_moves_without_a_tsr_smear() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    // TSR on, as the editor's Project Settings write it.
    let rendering = {
        use engine_state::settings::{global_config, NS_PROJECT};
        global_config()
            .owner_handle(NS_PROJECT, "rendering")
            .or_else(|| {
                engine_state::register_default_settings();
                global_config().owner_handle(NS_PROJECT, "rendering")
            })
            .expect("the project's rendering settings are registered")
    };
    rendering
        .set("tsr_quality", "quality")
        .expect("tsr_quality is a project setting");
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    let start = harness.camera;
    let step = (radius * 4.0).max(3.0) as f64 * 0.02;
    let at = |i: usize| EditorCameraState {
        position: [
            start.position[0] + step * i as f64,
            start.position[1],
            start.position[2] - step * i as f64,
        ],
        ..start
    };
    let view = harness.texture.create_view(&Default::default());
    let render = |renderer: &mut HelioRenderer, camera: EditorCameraState| {
        renderer.set_editor_camera_state(camera);
        renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
        renderer.render_frame(&harness.device, &harness.queue, &view, SIZE, SIZE, FORMAT);
        harness
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
    };
    const STEPS: usize = 12;
    let settled_at = |state: &LevelEditorState, camera: EditorCameraState| {
        let mut renderer = harness.renderer(state);
        for _ in 0..settle_frames().max(8) {
            render(&mut renderer, camera);
        }
        harness.read_texture(&harness.texture, wgpu::TextureAspect::All)
    };
    let moved_in = |state: &LevelEditorState| {
        let mut renderer = harness.renderer(state);
        for _ in 0..settle_frames().max(8) {
            render(&mut renderer, at(0));
        }
        for i in 1..=STEPS {
            render(&mut renderer, at(i));
        }
        harness.read_texture(&harness.texture, wgpu::TextureAspect::All)
    };
    let differs = |a: &[u8], b: &[u8]| {
        (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > PIXEL_THRESHOLD
    };
    // The share of the pixels showing the content (`with` against
    // `without`, settled at the last camera) that the moving frame misses.
    let lag = |name: &str, with: &LevelEditorState, without: &LevelEditorState| {
        let moved = moved_in(with);
        let settled = settled_at(with, at(STEPS));
        let empty = settled_at(without, at(STEPS));
        let region: Vec<bool> = settled
            .chunks_exact(4)
            .zip(empty.chunks_exact(4))
            .map(|(a, b)| differs(a, b))
            .collect();
        let shown = region.iter().filter(|r| **r).count();
        let missed = moved
            .chunks_exact(4)
            .zip(settled.chunks_exact(4))
            .zip(&region)
            .filter(|((a, b), r)| **r && differs(a, b))
            .count();
        if let Some(dir) = std::env::var_os("RENDER_ACCEPTANCE_DUMP_DIR") {
            for (label, pixels) in [("moved", &moved), ("settled", &settled), ("empty", &empty)] {
                let path = std::path::Path::new(&dir).join(format!("tsr_{name}_{label}.png"));
                image::save_buffer(&path, pixels, SIZE, SIZE, image::ColorType::Rgba8)
                    .expect("write dump");
            }
        }
        (missed, shown)
    };
    // The far portal kilometres away, a mesh on each side of it: they show
    // only through the near one.
    let far_side = |near_id: i32, far_id: i32| {
        let mut state = portal_scene_at(radius, near_id, far_id, false, 5000.0);
        for z in [radius * 2.0, -radius * 2.0] {
            let mesh = drop_mesh(&mut state);
            move_to(&mut state, &mesh, [5000.0, 0.0, z]);
        }
        state
    };
    let through = lag("through", &far_side(5, 5), &far_side(1, 2));
    // The control: what the portal shows, seen directly where it shows it
    // (the mesh beyond the opening; the one on the camera's side of the far
    // portal maps in front of the near one, which clips it).
    let direct = {
        let mut state = LevelEditorState::new();
        let mesh = drop_mesh(&mut state);
        move_to(&mut state, &mesh, [0.0, 0.0, -radius * 2.0]);
        lag("direct", &state, &LevelEditorState::new())
    };
    rendering
        .set("tsr_quality", "auto")
        .expect("tsr_quality is a project setting");
    let share = |(missed, shown): (usize, usize)| missed as f64 / shown.max(1) as f64;
    println!(
        "PORTAL TSR motion: through the portal {} of {} pixels differ from the settled frame ({:.3}); directly {} of {} ({:.3})",
        through.0,
        through.1,
        share(through),
        direct.0,
        direct.1,
        share(direct)
    );
    assert!(through.1 > 100, "the portal shows nothing: {through:?}");
    assert!(direct.1 > 100, "the control shows nothing: {direct:?}");
    assert!(
        share(through) <= share(direct) * 1.5 + 0.02,
        "portal content lags its direct motion under TSR (a smear): {through:?} against {direct:?}"
    );
}

/// An object with one typed component at `position`, by the editor's command.
fn add_with_component(
    state: &mut LevelEditorState,
    name: &str,
    position: [f32; 3],
    component: ui_level_editor::commands::TypedComponent,
) -> String {
    let added = execute_command(
        state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: name.to_string(),
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
    if position != [0.0; 3] {
        move_to(state, &id, position);
    }
    id
}

/// A blue albedo decal whose box holds a mesh of `radius` at `position`.
fn add_blue_decal(state: &mut LevelEditorState, radius: f32, position: [f32; 3]) -> String {
    use installed_plugins::decals_plugin::decals_component::{DecalComponent, DecalLayers};
    let decal = DecalComponent {
        size: [radius * 3.0; 3],
        color: [0.0, 0.2, 1.0],
        layers: DecalLayers::Albedo,
        ..Default::default()
    };
    add_with_component(
        state,
        "Decal",
        position,
        ui_level_editor::commands::TypedComponent::new(decal),
    )
}

/// What a portal shows is drawn by every drawer of the frame in the portal's
/// own view (`helio_core::view`): the decals of the far side show through it.
/// The portal used to duplicate meshes only, with a program of its own; a
/// decal there was invisible through the opening. Both frames show the far
/// side's meshes through the linked portals; only the decals differ, and
/// they change colour, not depth.
#[test]
fn a_portal_shows_the_decals_of_the_far_side() {
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
    // The far side well out of the camera's view: only the portal shows it.
    let far = radius * 20.0;
    let scene = |decals: bool| {
        let mut state = portal_scene_at(radius, 5, 5, false, far);
        for z in [radius * 2.0, -radius * 2.0] {
            let mesh = drop_matte_mesh(&mut state);
            move_to(&mut state, &mesh, [far, 0.0, z]);
            if decals {
                add_blue_decal(&mut state, radius, [far, 0.0, z]);
            }
        }
        add_light(
            &mut state,
            [far, radius * 1.5, 0.0],
            installed_plugins::lights_plugin::lights_component::LightType::Point,
            BRIGHT,
        );
        state
    };
    let render = |state: &LevelEditorState| {
        let mut renderer = harness.renderer(state);
        harness.frames_at_least(&mut renderer, || {}, 32)
    };
    let without = render(&scene(false));
    let with = render(&scene(true));
    with.dump("portal_decals");
    let change = with.difference(&without);
    println!("PORTAL DECALS vs without: {change:?}");
    assert!(
        change.color_pixels > 0,
        "the far side's decals did not show through the portal: {change:?}"
    );
    assert_eq!(change.depth_texels, 0, "a decal moved geometry");
}

/// Foliage shows through a portal: the far side's meadow is placed around the
/// portal view's own eye and drawn by the foliage drawer in that view
/// (`helio_core::view`). The portal used to duplicate meshes only. Both
/// frames look through the same linked portals; only the meadow beside the
/// far one differs, and its blades are depth.
#[test]
fn a_portal_shows_the_foliage_of_the_far_side() {
    use installed_plugins::foliage_plugin::foliage_component::FoliageComponent;
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
    // Blades are half a metre tall: the camera stands 4 m from the portal.
    let radius = 1.0;
    let harness = Harness::new(device, queue, radius);
    let settled = |state: &LevelEditorState| {
        let mut renderer = harness.renderer(state);
        // Placement fills a bounded number of tiles a frame, in each view.
        for _ in 0..5 {
            harness.frames(&mut renderer, || {});
        }
        harness.frames(&mut renderer, || {})
    };
    let scene = |meadow: bool| {
        let mut state = portal_scene_at(radius, 5, 5, false, 100.0);
        if meadow {
            let mut foliage = FoliageComponent::default();
            foliage.general.enabled = true;
            foliage.placement.layer_extent = 20.0;
            add_with_component(
                &mut state,
                "meadow",
                [100.0, 0.0, 0.0],
                ui_level_editor::commands::TypedComponent::new(foliage),
            );
        }
        state
    };
    let without = settled(&scene(false));
    let with = settled(&scene(true));
    with.dump("portal_foliage");
    with.dump_depth_change(&without, "portal_foliage_depth");
    let change = with.difference(&without);
    println!("PORTAL FOLIAGE vs without: {change:?}");
    assert!(
        change.depth_texels > 0,
        "the far side's foliage did not show through the portal: {change:?}"
    );
}

/// A water surface reflects the frame as a mirrored capture view, drawn by
/// every drawer (`helio_core::view`): a mesh high above the camera's view,
/// and a decal on it, show in the water without changing the frame's depth.
/// The capture used to be a pass list of the meshes' own; a decal never
/// showed in it.
#[test]
fn a_water_reflection_shows_meshes_and_decals_out_of_view() {
    use installed_plugins::water_plugin::water_component::WaterVolumeComponent;
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
    // Far ahead of the camera, 18 degrees over the horizon: past the top of
    // its view (45 degrees tall, pitched down by about 9.5), its mirror image
    // about 21 degrees under the horizon, inside it.
    let distance = (radius * 4.0).max(3.0);
    let ahead = distance * 10.0;
    let above = [
        0.0,
        distance / 6.0 + ahead * 18f32.to_radians().tan(),
        distance - ahead,
    ];
    let scene = |mesh: bool, decal: bool| {
        let mut state = LevelEditorState::new();
        let mut water = WaterVolumeComponent::default();
        water.size = [distance * 40.0, radius, distance * 40.0];
        water.bounded = true;
        // The surface at the rest plane the capture mirrors about
        // (`WaterSimPass::set_ocean_rest_height`, 0 unless a host sets it):
        // the surface draws at its owner's height.
        add_with_component(
            &mut state,
            "Water",
            [0.0; 3],
            ui_level_editor::commands::TypedComponent::new(water),
        );
        add_light(
            &mut state,
            [0.0, above[1] + radius * 2.0, above[2] + radius * 3.0],
            installed_plugins::lights_plugin::lights_component::LightType::Point,
            BRIGHT,
        );
        if mesh {
            let id = drop_matte_mesh(&mut state);
            move_to(&mut state, &id, above);
            if decal {
                add_blue_decal(&mut state, radius, above);
            }
        }
        state
    };
    let render = |state: &LevelEditorState| {
        let mut renderer = harness.renderer(state);
        harness.frames(&mut renderer, || {})
    };
    let empty = render(&scene(false, false));
    let mesh = render(&scene(true, false));
    let decal = render(&scene(true, true));
    mesh.dump("water_reflects_mesh");
    decal.dump("water_reflects_decal");
    let reflected = mesh.difference(&empty);
    println!("REFLECTED MESH vs none: {reflected:?}");
    assert_eq!(
        reflected.depth_texels, 0,
        "the mesh is in the camera's view: the test sees it directly"
    );
    assert!(
        reflected.color_pixels > 0,
        "the water does not reflect the mesh above it: {reflected:?}"
    );
    let decorated = decal.difference(&mesh);
    println!("REFLECTED DECAL vs without: {decorated:?}");
    assert_eq!(decorated.depth_texels, 0, "a decal moved geometry");
    assert!(
        decorated.color_pixels > 0,
        "the reflection does not show the mesh's decal: {decorated:?}"
    );
}

/// What a portal shows keeps its materials when the scene grows. The material rows
/// are the scene join's output and are reallocated as meshes are added; a portal
/// draw still bound to the old buffer shows garbage (the content looked flat and
/// blue). The meshes here are emissive orange, so what comes through the portal
/// must stay orange (red well above blue) after the scene has grown.
#[test]
fn portal_content_keeps_its_materials_when_the_scene_grows() {
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

    let unlinked = {
        let state = portal_scene(radius, 1, 2, true);
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {})
    };
    // Linked portals and, to start with, nothing to look at.
    let mut state = portal_scene(radius, 5, 5, false);
    let mut renderer = harness.renderer(&state);
    harness.frames(&mut renderer, || {});

    // The scene grows well past the material buffer's capacity (far out of view)...
    for n in 0..64 {
        let mesh = drop_mesh(&mut state);
        move_to(&mut state, &mesh, [2000.0 + n as f32 * 10.0, 0.0, 0.0]);
    }
    harness.frames(&mut renderer, || {});
    // ...and only then does the content beside the far portal arrive, so its
    // material rows live in the reallocated buffer.
    add_portal_content(&mut state, radius);
    let shown = harness.frames(&mut renderer, || {});

    let [red, _, blue] = mean_color_of_difference(&shown, &unlinked)
        .expect("the linked portal shows the content that arrived after the growth");
    assert!(
        red > blue + 10.0,
        "the content that arrived after the scene grew lost its materials through the \
         portal (mean r {red}, b {blue}); it should be emissive orange"
    );
}

/// Mean `[r, g, b]` (0-255) and count of the strongly red pixels of `frame`: the
/// red mesh, without the sky and the ground line around it.
fn mean_of_red_pixels(frame: &Frame) -> Option<([f32; 3], usize)> {
    let mut sum = [0.0f64; 3];
    let mut count = 0usize;
    for p in frame.color.chunks_exact(4) {
        if p[0] > 120
            && u32::from(p[0]) > 2 * u32::from(p[1]) + 40
            && u32::from(p[0]) > 2 * u32::from(p[2]) + 40
        {
            for i in 0..3 {
                sum[i] += f64::from(p[i]);
            }
            count += 1;
        }
    }
    (count > 0).then(|| (sum.map(|v| (v / count as f64) as f32), count))
}

fn drop_red_mesh(state: &mut LevelEditorState) -> String {
    drop_mesh_with(
        state,
        json!({"base_color": [0.9, 0.05, 0.05, 1.0], "metallic": 0.0, "roughness": 0.8,
               "emissive_color": [0.0, 0.0, 0.0], "emissive_intensity": 0.0, "alpha": 1.0}),
    )
}

/// DIAGNOSTIC: does a lit matte mesh, seen directly and through a linked portal,
/// respond to the sun? Prints the mean colour of what changed.
#[test]
#[ignore = "diagnostic: prints and dumps frames"]
fn diag_portal_lighting() {
    let Some((device, queue)) = device() else {
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);

    for sun_on in [true, false] {
        let environment = |state: &mut LevelEditorState| {
            if sun_on {
                add_light(
                    state,
                    [1.0e4, 0.0, 0.0],
                    installed_plugins::lights_plugin::lights_component::LightType::Directional,
                    SUN,
                );
            }
            assert!(plugin_kinds::sky::create_sky(state).changed);
        };
        let render = |state: &LevelEditorState| {
            let mut renderer = harness.renderer(state);
            harness.frames(&mut renderer, || {})
        };
        // Direct.
        let mut empty = LevelEditorState::new();
        environment(&mut empty);
        let mut direct = LevelEditorState::new();
        let mesh = drop_red_mesh(&mut direct);
        move_to(&mut direct, &mesh, [0.0, 0.0, -radius * 2.0]);
        environment(&mut direct);
        let (_empty_f, direct_f) = (render(&empty), render(&direct));
        direct_f.dump(&format!("diag_direct_sun{sun_on}"));
        // Through a portal.
        let portal = |link: i32| {
            let mut state = portal_scene(radius, link, if link == 5 { 5 } else { 6 }, false);
            for z in [radius * 2.0, -radius * 2.0] {
                let mesh = drop_red_mesh(&mut state);
                move_to(&mut state, &mesh, [100.0, 0.0, z]);
            }
            environment(&mut state);
            state
        };
        let (_unlinked, linked) = (render(&portal(1)), render(&portal(5)));
        linked.dump(&format!("diag_portal_sun{sun_on}"));
        println!(
            "DIAG sun={sun_on}: direct {:?}  portal {:?}",
            mean_of_red_pixels(&direct_f),
            mean_of_red_pixels(&linked)
        );
    }
}

/// DIAGNOSTIC: GPU time with no portals and with a linked pair in view: the
/// primary view's passes and each secondary view's (`helio_core::view`), over
/// pipelined frames (no wait between them), unlinked and linked interleaved.
#[test]
#[ignore = "diagnostic: prints GPU timings"]
fn diag_portal_cost() {
    let Some((device, queue)) = device() else {
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    let view = harness.texture.create_view(&Default::default());
    for round in 0..3 {
        for (label, near, far) in [("unlinked", 1, 2), ("linked", 5, 5)] {
            let state = portal_scene(radius, near, far, true);
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            let (mut primary, mut views) = (Vec::new(), Vec::new());
            for _ in 0..8 {
                // Eight frames in flight, then their timings.
                for _ in 0..8 {
                    renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
                    renderer.render_frame(
                        &harness.device,
                        &harness.queue,
                        &view,
                        SIZE,
                        SIZE,
                        FORMAT,
                    );
                }
                harness
                    .device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .unwrap();
                // The passes' own times: the graph's span (`__graph_*`)
                // also holds the time the CPU spent recording the views.
                if let Some((_, passes)) = renderer.gpu_timings() {
                    primary.push(
                        passes
                            .iter()
                            .filter(|(name, _)| !name.starts_with("__"))
                            .map(|(_, ms)| ms)
                            .sum(),
                    );
                }
                views.push(
                    renderer
                        .view_gpu_timings()
                        .iter()
                        .map(|(_, _, ms)| ms)
                        .sum::<f32>(),
                );
            }
            let median = |v: &mut Vec<f32>| {
                v.sort_by(f32::total_cmp);
                v.get(v.len() / 2).copied().unwrap_or(f32::NAN)
            };
            println!(
                "DIAG round {round} {label}: primary {:.3} ms, views {:.3} ms ({} views)",
                median(&mut primary),
                median(&mut views),
                renderer.view_gpu_timings().len()
            );
        }
    }
}

/// The water's reflection capture costs only while water is in view: the GPU
/// time of the frame's passes and its views with a water volume in front of
/// the camera, and the same volume behind it (the capture not scheduled),
/// interleaved, over pipelined frames.
#[test]
#[ignore = "diagnostic: prints GPU timings"]
fn diag_water_capture_cost() {
    use installed_plugins::water_plugin::water_component::WaterVolumeComponent;
    let Some((device, queue)) = device() else {
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    let view = harness.texture.create_view(&Default::default());
    let distance = (radius * 4.0).max(3.0);
    let scene = |z: f32| {
        let mut state = LevelEditorState::new();
        let mut water = WaterVolumeComponent::default();
        water.size = [distance * 4.0, radius, distance * 4.0];
        water.bounded = true;
        add_with_component(
            &mut state,
            "Water",
            [0.0, 0.0, z],
            ui_level_editor::commands::TypedComponent::new(water),
        );
        let id = drop_matte_mesh(&mut state);
        move_to(&mut state, &id, [0.0, radius, -distance]);
        state
    };
    for round in 0..3 {
        // Ahead of the camera (it looks down -Z), then far behind it.
        for (label, z) in [("in view", -distance * 2.0), ("behind", distance * 200.0)] {
            let state = scene(z);
            let mut renderer = harness.renderer(&state);
            harness.frames(&mut renderer, || {});
            let (mut primary, mut views) = (Vec::new(), Vec::new());
            for _ in 0..8 {
                for _ in 0..8 {
                    renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
                    renderer.render_frame(
                        &harness.device,
                        &harness.queue,
                        &view,
                        SIZE,
                        SIZE,
                        FORMAT,
                    );
                }
                harness
                    .device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .unwrap();
                if let Some((_, passes)) = renderer.gpu_timings() {
                    primary.push(
                        passes
                            .iter()
                            .filter(|(name, _)| !name.starts_with("__"))
                            .map(|(_, ms)| ms)
                            .sum(),
                    );
                }
                views.push(
                    renderer
                        .view_gpu_timings()
                        .iter()
                        .map(|(_, _, ms)| ms)
                        .sum::<f32>(),
                );
            }
            let median = |v: &mut Vec<f32>| {
                v.sort_by(f32::total_cmp);
                v.get(v.len() / 2).copied().unwrap_or(f32::NAN)
            };
            println!(
                "DIAG round {round} water {label}: primary {:.3} ms, views {:.3} ms ({} views)",
                median(&mut primary),
                median(&mut views),
                renderer.view_gpu_timings().len()
            );
        }
    }
}

fn rotate_y(state: &mut LevelEditorState, id: &str, degrees: f32) {
    if degrees == 0.0 {
        return;
    }
    let result = execute_command(
        state,
        SceneCommand::SetTransform {
            id: id.to_string(),
            position: None,
            rotation: Some([0.0, degrees, 0.0]),
            scale: None,
        },
    );
    assert!(result.changed, "rotation edit was refused");
}

/// What a portal shows is lit by the lights of the world it shows, not of the
/// world it is seen from. The far portal is far enough from the near one that a
/// light beside the content is nowhere near the place the content appears
/// through the near portal, and the other way round.
#[test]
fn portal_content_is_lit_by_the_lights_of_its_own_world() {
    use engine_backend::scene::frames::set_rigid_frame;

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
    let far_x = 5000.0;
    // `None`: no light; otherwise a light at that x, in front of the content
    // (which sits beside the far portal), reaching about 1200 m.
    let render = |light_x: Option<f32>, matches_space: bool| {
        let mut state = LevelEditorState::new();
        // Allocate the relevant light after the old portal shader's 256-row
        // scan limit. These lights cannot reach either portal or its content.
        for index in 0..256 {
            let light = add_light(
                &mut state,
                [10_000.0 + index as f32 * 20.0, 0.0, 0.0],
                installed_plugins::lights_plugin::lights_component::LightType::Point,
                BRIGHT,
            );
            set_light(&mut state, &light, "range", Box::new(10.0f32));
            set_light(&mut state, &light, "cast_shadows", Box::new(false));
        }
        let frame = add_object(&mut state, "Destination frame", ObjectType::Folder);
        let parent_to_destination = |state: &mut LevelEditorState, id: String| {
            let result = execute_command(
                state,
                SceneCommand::ReparentObject {
                    id,
                    new_parent_id: Some(frame.clone()),
                },
            );
            assert!(
                result.changed,
                "the object was not placed in the destination"
            );
        };
        let entity = state.scene.world().entity_for(&frame).expect("destination");
        set_rigid_frame(&mut state.scene.world_mut(), entity, true).expect("a free frame slot");

        // The near portal faces the camera.
        let near = add_portal(&mut state, radius, "near", [0.0; 3], 5);
        let far = add_portal(&mut state, radius, "far", [far_x, 0.0, 0.0], 5);
        parent_to_destination(&mut state, far);
        rotate_y(&mut state, &near, 180.0);
        for z in [radius * 2.0, -radius * 2.0] {
            let mesh = drop_red_mesh(&mut state);
            move_to(&mut state, &mesh, [far_x, 0.0, z]);
            parent_to_destination(&mut state, mesh);
        }
        if let Some(x) = light_x {
            let light = add_light(
                &mut state,
                [x, radius * 1.5, radius * 4.0],
                installed_plugins::lights_plugin::lights_component::LightType::Point,
                BRIGHT,
            );
            set_light(&mut state, &light, "range", Box::new(1200.0f32));
            set_light(&mut state, &light, "cast_shadows", Box::new(false));
            if matches_space {
                parent_to_destination(&mut state, light);
            }
        }
        let mut renderer = harness.renderer(&state);
        harness.frames(&mut renderer, || {})
    };
    let brightness = |frame: &Frame| {
        frame
            .color
            .chunks_exact(4)
            .map(|p| f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2]))
            .sum::<f64>()
            / (frame.color.len() / 4) as f64
    };
    let dark = brightness(&render(None, false));
    let beside_content = brightness(&render(Some(far_x), true)) - dark;
    let wrong_space = brightness(&render(Some(far_x), false)) - dark;
    let beside_portal = brightness(&render(Some(0.0), false)) - dark;
    println!(
        "lights: beside content {beside_content:+.2}, beside portal {beside_portal:+.2} (dark {dark:.2})"
    );
    assert!(
        beside_content > 2.0,
        "a light beside the content did not light it through the portal \
         (brightness {beside_content:+.2} over {dark:.2})"
    );
    assert!(
        beside_portal < beside_content * 0.25,
        "a light beside the portal lit the content it shows, which is far from it \
         ({beside_portal:+.2} against {beside_content:+.2} for a light beside the content)"
    );
    assert!(
        wrong_space.abs() < beside_content * 0.25,
        "a light at the same XYZ lit portal content from the wrong space \
         ({wrong_space:+.2} against {beside_content:+.2})"
    );
}

#[test]
fn portal_aperture_clips_direct_billboards_and_shows_destination_billboards() {
    use installed_plugins::lights_plugin::lights_component::LightType;

    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    let radius = typed_mesh().bounds_local[3];
    let harness = Harness::new(device, queue, radius);
    let far_x = 5000.0;
    let render = |positions: &[[f32; 3]]| {
        let mut state = portal_scene_at(radius, 5, 5, false, far_x);
        for &position in positions {
            let id = add_light(&mut state, position, LightType::Point, 0.0);
            set_light(&mut state, &id, "cast_shadows", Box::new(false));
        }
        harness.frames(&mut harness.renderer(&state), || {})
    };
    let empty = render(&[]);
    let foreground = render(&[[0.0, 0.0, radius]]).difference(&empty);
    let behind = render(&[[0.0, 0.0, -radius]]).difference(&empty);
    let destination = render(&[[far_x, 0.0, radius], [far_x, 0.0, -radius]]).difference(&empty);
    assert!(
        foreground.color_pixels > 0,
        "foreground icon missing: {foreground:?}"
    );
    assert_eq!(
        behind.color_pixels, 0,
        "icon leaked past the plane: {behind:?}"
    );
    assert!(
        destination.color_pixels > 0,
        "destination icon missing: {destination:?}"
    );
}

/// The sky, wind and sprite plugins' create entries, through the shell's
/// generic add path (their editor crates depend on the shell, so its tests
/// do not link them; `installed_plugins`' `tests/level_editor.rs` tests the
/// entries themselves).
mod plugin_kinds {
    use installed_plugins::foliage_plugin::foliage_component::WindComponent;
    use installed_plugins::sprites_plugin::sprites_component::SpriteComponent;
    use ui_level_editor::commands::{CommandResult, TypedComponent};
    use ui_level_editor::extend::add_object;
    use ui_level_editor::scene_edit::{ObjectType, Transform};
    use ui_level_editor::LevelEditorState;

    pub mod sky {
        use installed_plugins::sky_plugin::sky_component::{level_atmosphere, AtmosphereComponent};
        use ui_level_editor::commands::{CommandResult, TypedComponent};
        use ui_level_editor::extend::{level_component, LevelComponent};
        use ui_level_editor::LevelEditorState;

        pub const SKY_OBJECT_NAME: &str = "Sky";

        pub fn level_sky(world: &pulsar_scenedb::World) -> Option<LevelComponent> {
            level_component(world, level_atmosphere)
        }

        pub fn create_sky(state: &mut LevelEditorState) -> CommandResult {
            super::add_unselected(
                state,
                SKY_OBJECT_NAME,
                vec![TypedComponent::new(AtmosphereComponent::default())],
            )
        }
    }

    pub fn create_wind(state: &mut LevelEditorState) -> CommandResult {
        add_unselected(
            state,
            "Wind",
            vec![TypedComponent::new(WindComponent::default())],
        )
    }

    /// An object holding `components`, added as one undoable command and
    /// not selected (what the World Settings' create buttons did here).
    fn add_unselected(
        state: &mut LevelEditorState,
        name: &str,
        components: Vec<TypedComponent>,
    ) -> CommandResult {
        use ui_level_editor::commands::{execute_command, SceneCommand};
        execute_command(
            state,
            SceneCommand::AddObjectWithComponents {
                data: ui_level_editor::SceneObjectData {
                    id: String::new(),
                    name: name.to_string(),
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
                components,
            },
        )
    }

    pub fn create_sprite(
        state: &mut LevelEditorState,
        position: [f32; 2],
        sprite: SpriteComponent,
    ) -> CommandResult {
        add_object(
            state,
            "Sprite",
            ObjectType::Empty,
            Transform {
                position: [f64::from(position[0]), f64::from(position[1]), 0.0],
                ..Transform::default()
            },
            vec![TypedComponent::new(sprite)],
        )
    }
}

/// Pixels of `frame` that differ from `reference` and are the selection
/// outline's yellow (`helio-pass-selection-outline`, drawn at full strength
/// where the selected surface is in front).
fn outline_pixels(frame: &Frame, reference: &Frame) -> usize {
    frame
        .color
        .chunks_exact(4)
        .zip(reference.color.chunks_exact(4))
        .filter(|(px, before)| {
            px != before && px[0] >= 230 && (160..=230).contains(&px[1]) && px[2] <= 60
        })
        .count()
}

#[test]
fn a_selected_mesh_is_outlined_in_the_editor_and_the_outline_leaves_with_the_selection() {
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

    let mut state = LevelEditorState::new();
    let id = drop_matte_mesh(&mut state);
    state.scene.select_object(None);
    let mut renderer = harness.renderer(&state);
    let unselected = harness.frames(&mut renderer, || {});

    state.scene.select_object(Some(id.clone()));
    let selected = harness.frames(&mut renderer, || {});
    selected.dump("selection_outline_selected");
    let outlined = outline_pixels(&selected, &unselected);
    println!("SELECTION outline pixels: {outlined}");
    assert!(
        outlined >= 32,
        "a selected mesh shows no outline ({outlined} outline pixels)"
    );

    state.scene.select_object(None);
    let deselected = harness.frames(&mut renderer, || {});
    let left = outline_pixels(&deselected, &unselected);
    println!("SELECTION outline pixels after deselect: {left}");
    assert!(
        left * 10 < outlined,
        "the outline stays after the selection is cleared ({left} of {outlined})"
    );
}
