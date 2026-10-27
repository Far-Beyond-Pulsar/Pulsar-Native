//! Virtual geometry end to end, headless (plan
//! `.agents/plans/virtual-geometry.md`, T10): a dense mesh with a
//! `VirtualGeometryComponent` is built into a `.vgeo` in the background,
//! streamed in, and then drawn by the cluster pipeline instead of the classic
//! mesh path, through the editor's real `HelioRenderer`.
//!
//! * `virtual_geometry_matches_the_classic_render_and_off_restores_it` renders
//!   the same dense sphere classically, VG-active (compared within a
//!   tolerance), VG with `force_root_only` (must visibly differ, proving the
//!   cluster path is what draws), and with VG globally Off (equal to classic
//!   again).
//! * `authored_lod_is_ignored_while_virtual_geometry_is_active` puts a
//!   `LODComponent` on the sphere that would swap it for a cube at distance:
//!   VG-active keeps the sphere, Off applies the cube.
//! * `every_mode_renders_without_validation_errors` cycles Auto,
//!   ForceNonRtFallback, ForceHwRaster and Off under a validation error scope.
//!
//! No adapter, or one without the virtual-geometry device limits: skipped.
//! Run with `--nocapture` to see the measured differences; set
//! `RENDER_ACCEPTANCE_DUMP_DIR` to write the frames as PNGs.

use std::time::{Duration, Instant};

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use helio_component::components::{
    MeshAssetPath, MeshGeometryKey, StaticMeshComponent, StaticMeshMaterialSlot,
    StaticMeshMaterialSlots, VirtualGeometryComponent,
};
use helio_component::mesh_cache::MeshSection;
use pulsar_settings::project::virtual_geometry::{VirtualGeometryMode, VirtualGeometrySettings};
use serde_json::json;
use std::collections::HashMap;
use ui_level_editor::commands::{execute_command, SceneCommand};
use ui_level_editor::scene_edit::components;
use ui_level_editor::scene_edit::{MeshType, ObjectType, Transform};
use ui_level_editor::{LevelEditorState, SceneObjectData};

const SIZE: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const PIXEL_THRESHOLD: u32 = 48;
const RADIUS: f32 = 40.0;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let features = adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()
}

fn mesh_asset_named(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/meshes/primitives")
        .join(name)
        .canonicalize()
        .expect("primitive mesh is checked in");
    let path = path.to_string_lossy();
    path.strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .replace('\\', "/")
}

/// A UV sphere of `2 * rings * segments` triangles.
fn dense_sphere(
    rings: u32,
    segments: u32,
    radius: f32,
) -> (Vec<helio_core::PackedVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let theta = std::f32::consts::PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let normal = glam::Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            let tangent = glam::Vec3::new(-phi.sin(), 0.0, phi.cos());
            vertices.push(helio_core::PackedVertex::from_components(
                (normal * radius).into(),
                normal.into(),
                [segment as f32 / segments as f32, ring as f32 / rings as f32],
                tangent.into(),
                1.0,
            ));
        }
    }
    let mut indices = Vec::new();
    let stride = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let b = a + stride;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (vertices, indices)
}

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

/// A generated (never loaded from disk) dense sphere with one emissive slot.
fn dense_mesh() -> StaticMeshComponent {
    let (vertices, indices) = dense_sphere(120, 160, RADIUS);
    let content: u128 = 0x5eed_c0de_0000_0000_0000_0000_d3a5_e001;
    StaticMeshComponent {
        mesh_asset: MeshAssetPath::generated(content),
        geometry_key: MeshGeometryKey(content),
        material_slots: StaticMeshMaterialSlots {
            slots: vec![StaticMeshMaterialSlot {
                name: "dense".into(),
                surface_override: Some(emissive_surface()),
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
        bounds_local: [0.0, 0.0, 0.0, RADIUS],
        ..Default::default()
    }
}

#[derive(Clone)]
struct Frame {
    color: Vec<u8>,
    depth: Vec<f32>,
}

#[derive(Debug, Clone, Copy)]
struct Difference {
    color_pixels: usize,
    depth_texels: usize,
    /// Pixels of `self` that are not the background.
    covered: usize,
}

impl Frame {
    fn difference(&self, other: &Frame) -> Difference {
        let color_pixels = self
            .color
            .chunks_exact(4)
            .zip(other.color.chunks_exact(4))
            .filter(|(a, b)| {
                (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > PIXEL_THRESHOLD
            })
            .count();
        let depth_texels = self
            .depth
            .iter()
            .zip(&other.depth)
            .filter(|(a, b)| (**a - **b).abs() > 1e-3 * a.abs().max(b.abs()))
            .count();
        let background = &self.color[..4];
        let covered = self
            .color
            .chunks_exact(4)
            .filter(|px| *px != background)
            .count();
        Difference {
            color_pixels,
            depth_texels,
            covered,
        }
    }

    fn dump(&self, name: &str) {
        let Some(dir) = std::env::var_os("RENDER_ACCEPTANCE_DUMP_DIR") else {
            return;
        };
        let path = std::path::Path::new(&dir).join(format!("vg_{name}.png"));
        image::save_buffer(&path, &self.color, SIZE, SIZE, image::ColorType::Rgba8)
            .expect("write dump");
    }
}

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    camera: EditorCameraState,
}

impl Harness {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vg-acceptance-target"),
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
        let distance = (RADIUS * 4.0) as f64;
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
        }
    }

    fn renderer(&self, state: &LevelEditorState) -> HelioRenderer {
        let mut renderer = HelioRenderer::new(state.scene.shared_scene());
        renderer.set_editor_camera_state(self.camera);
        renderer
    }

    /// Renders `frames` frames and reads the last one back.
    fn frames(&self, renderer: &mut HelioRenderer, frames: usize) -> Frame {
        let view = self.texture.create_view(&Default::default());
        let mut encoded = false;
        for frame in 0..frames {
            // Alternate a tiny camera nudge: a still camera lets the renderer
            // skip frames, and the nudge ends at the base pose.
            let yaw = self.camera.yaw + if frame % 2 == 0 { 1e-3 } else { 0.0 };
            renderer.set_editor_camera_state(EditorCameraState { yaw, ..self.camera });
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
            depth: self
                .read_texture(depth, wgpu::TextureAspect::DepthOnly)
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        }
    }

    fn read_texture(&self, texture: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Vec<u8> {
        let size = texture.size();
        let row = size.width * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vg-acceptance-readback"),
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

    /// Render until the virtual-geometry runtime has an asset ready (the
    /// `.vgeo` is built on a worker thread), then settle.
    fn until_ready(&self, renderer: &mut HelioRenderer) -> Frame {
        let start = Instant::now();
        loop {
            let frame = self.frames(renderer, 4);
            if renderer
                .virtual_geometry_runtime()
                .expect("the viewport renderer is built")
                .any_ready()
            {
                // The instance derivation and the cull see it a couple of
                // frames later.
                return self.frames(renderer, 12);
            }
            assert!(
                start.elapsed() < Duration::from_secs(240),
                "the virtual-geometry asset never became ready"
            );
            drop(frame);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn vg_settings(mode: VirtualGeometryMode) -> VirtualGeometrySettings {
    VirtualGeometrySettings {
        mode,
        ..Default::default()
    }
}

/// A scene with the dense sphere, optionally carrying the VG component. The
/// object is made by the editor's own command, so it has an id for the
/// by-name component functions.
fn scene(vg: Option<VirtualGeometryComponent>) -> (LevelEditorState, String) {
    let mut state = LevelEditorState::new();
    let id = execute_command(
        &mut state,
        SceneCommand::AddObject {
            data: SceneObjectData {
                id: String::new(),
                name: "dense sphere".to_string(),
                object_type: ObjectType::Mesh(MeshType::Custom),
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
    )
    .affected_ids
    .first()
    .cloned()
    .expect("AddObject creates an object");
    let object = {
        let mut world = state.scene.world_mut();
        let object = world.entity_for(&id).expect("object exists");
        pulsar_world_registry::attach_value(&mut world, object, dense_mesh()).expect("attach mesh");
        if let Some(vg) = vg {
            pulsar_world_registry::attach_value(&mut world, object, vg)
                .expect("attach VG component");
        }
        object
    };
    let _ = object;
    (state, id)
}

fn setup() -> Option<Harness> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return None;
    };
    let limits = device.limits();
    if limits.max_storage_buffers_per_shader_stage < 20
        || limits.max_storage_textures_per_shader_stage < 8
    {
        eprintln!("skipping: the adapter lacks the virtual-geometry device limits");
        return None;
    }
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    Some(Harness::new(device, queue))
}

#[test]
fn virtual_geometry_matches_the_classic_render_and_off_restores_it() {
    let Some(harness) = setup() else { return };

    // Classic: no VirtualGeometryComponent, so the mesh never becomes VG-active.
    let (classic_state, _) = scene(None);
    let mut classic_renderer = harness.renderer(&classic_state);
    classic_renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Auto)));
    let classic = harness.frames(&mut classic_renderer, 12);
    classic.dump("classic");
    assert!(
        classic.difference(&classic).covered > 1000,
        "the classic sphere is not drawn"
    );
    assert!(!classic_renderer
        .virtual_geometry_runtime()
        .expect("the viewport renderer is built")
        .any_ready());

    // VG-active.
    let (state, _) = scene(Some(VirtualGeometryComponent::default()));
    let mut renderer = harness.renderer(&state);
    renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Auto)));
    let active = harness.until_ready(&mut renderer);
    active.dump("active");
    let d = active.difference(&classic);
    println!("VG active vs classic: {d:?}");
    // Same sphere within the pixel error: shading and silhouette agree except
    // along the edge where the cut is one pixel coarser and normals interpolate.
    assert!(
        d.color_pixels * 100 <= d.covered * 3,
        "VG differs from the classic render on too many pixels: {d:?}"
    );
    assert!(
        d.depth_texels * 100 <= d.covered * 5,
        "VG depth differs from the classic depth: {d:?}"
    );

    // The cluster path is what draws: the coarsest cut alone is a visibly
    // different (low-poly) sphere.
    let (root_state, _) = scene(Some(VirtualGeometryComponent {
        force_root_only: true,
        lod_error_scale: 16.0,
        ..VirtualGeometryComponent::default()
    }));
    let mut root_renderer = harness.renderer(&root_state);
    root_renderer.set_virtual_geometry_settings(Some(VirtualGeometrySettings {
        tau_pixels: 16.0,
        ..vg_settings(VirtualGeometryMode::Auto)
    }));
    let root_only = harness.until_ready(&mut root_renderer);
    root_only.dump("root_only");
    let r = root_only.difference(&classic);
    println!("VG root-only vs classic: {r:?}");
    // A coarse polygon inside the sphere: if the classic mesh were drawn too,
    // the silhouette would be the full sphere again and this would be ~0.
    assert!(
        r.depth_texels * 100 > r.covered || r.color_pixels * 100 > r.covered,
        "a coarse virtual-geometry cut looks identical to the classic sphere: the classic mesh is still drawing ({r:?})"
    );

    // Off: back to the classic path, same as the classic scene.
    renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Off)));
    let off = harness.frames(&mut renderer, 16);
    off.dump("off");
    let o = off.difference(&classic);
    println!("VG off vs classic: {o:?}");
    assert!(
        o.color_pixels * 200 <= o.covered && o.depth_texels * 200 <= o.covered,
        "with VG Off the mesh must draw as the classic path does: {o:?}"
    );
}

#[test]
fn authored_lod_is_ignored_while_virtual_geometry_is_active() {
    let Some(harness) = setup() else { return };
    let near_distance = glam::DVec3::from_array(harness.camera.position).length() as f32;
    let far = harness.camera.position[2] * 2.0;
    let far_camera = EditorCameraState {
        position: [0.0, far / 6.0, far],
        ..harness.camera
    };
    // `frames` resets the pose each frame, so the far view needs its own harness.
    let harness = Harness {
        device: harness.device.clone(),
        queue: harness.queue.clone(),
        texture: harness.texture.clone(),
        camera: far_camera,
    };

    let build = || {
        let (state, id) = scene(Some(VirtualGeometryComponent::default()));
        {
            let mut world = state.scene.world_mut();
            components::add_component(
                &mut world,
                &id,
                "LODComponent".to_string(),
                json!({
                    "lod_levels": [
                        { "distance_threshold": 0.0, "mesh_asset": "" },
                        { "distance_threshold": near_distance * 1.5, "mesh_asset": mesh_asset_named("SM_Cube.fbx") },
                    ],
                    "lod_bias": 0.0,
                }),
            );
        }
        state
    };

    // Reference views at the far camera: the plain sphere (classic) and the
    // authored cube (LOD applied, VG not active).
    let (sphere_state, _) = scene(None);
    let mut sphere_renderer = harness.renderer(&sphere_state);
    let sphere = harness.frames(&mut sphere_renderer, 12);

    // VG globally Off: the LOD applies, the cube replaces the sphere.
    let off_state = build();
    let mut off_renderer = harness.renderer(&off_state);
    off_renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Off)));
    let mut off = harness.frames(&mut off_renderer, 16);
    off.dump("lod_off");
    let cube_vs_sphere = off.difference(&sphere);
    println!("LOD applied (VG off) vs sphere: {cube_vs_sphere:?}");
    assert!(
        cube_vs_sphere.color_pixels * 100 > cube_vs_sphere.covered * 3,
        "the authored LOD (a cube) did not replace the sphere with VG Off: {cube_vs_sphere:?}"
    );

    // VG Auto: once the asset is ready the instance is VG-active and the LOD is inert.
    let state = build();
    let mut renderer = harness.renderer(&state);
    renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Auto)));
    let _ = harness.until_ready(&mut renderer);
    let active = harness.frames(&mut renderer, 16);
    active.dump("lod_active");
    let still_sphere = active.difference(&sphere);
    let not_cube = active.difference(&off);
    println!("LOD ignored (VG active) vs sphere: {still_sphere:?}; vs cube: {not_cube:?}");
    assert!(
        still_sphere.color_pixels * 100 <= still_sphere.covered * 3,
        "an active VG instance must keep the sphere, not the authored LOD cube: {still_sphere:?}"
    );

    // And turning VG off hands the instance back to its LOD.
    renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Off)));
    off = harness.frames(&mut renderer, 16);
    let back = off.difference(&sphere);
    println!("LOD back after VG Off vs sphere: {back:?}");
    assert!(
        back.color_pixels * 100 > back.covered * 3,
        "the authored LOD must apply again once VG is Off: {back:?}"
    );
}

#[test]
fn every_mode_renders_without_validation_errors() {
    let Some(harness) = setup() else { return };
    let (state, _) = scene(Some(VirtualGeometryComponent::default()));
    let mut renderer = harness.renderer(&state);
    renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Auto)));
    let _ = harness.until_ready(&mut renderer);

    let mut frames = Vec::new();
    for mode in [
        VirtualGeometryMode::Auto,
        VirtualGeometryMode::ForceNonRtFallback,
        VirtualGeometryMode::ForceHwRaster,
        VirtualGeometryMode::Off,
        VirtualGeometryMode::Auto,
    ] {
        let scope = harness
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.set_virtual_geometry_settings(Some(vg_settings(mode)));
        let frame = harness.frames(&mut renderer, 10);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{mode:?}: GPU validation error: {error:?}");
        let covered = frame.difference(&frame).covered;
        println!("{mode:?}: covered pixels {covered}");
        assert!(
            covered > 1000,
            "{mode:?}: the sphere vanished ({covered} pixels)"
        );
        frame.dump(&format!("mode_{}", mode.as_str()));
        frames.push(frame);
    }
    // Every mode draws the same sphere within the VG tolerance.
    let reference = frames[3].clone();
    for (index, frame) in frames.iter().enumerate() {
        let d = frame.difference(&reference);
        println!("mode #{index} vs Off: {d:?}");
        assert!(
            d.color_pixels * 100 <= d.covered * 5,
            "mode #{index} differs from classic: {d:?}"
        );
    }
}

/// Distinct colours over the covered (non-background) pixels, and the mean of
/// `red - blue` (0.5 = neutral) there: the heat ramps run blue (low) to red.
fn colour_stats(frame: &Frame) -> (usize, f32) {
    let background = &frame.color[..4];
    let mut colours = std::collections::HashSet::new();
    let (mut sum, mut count) = (0.0f64, 0usize);
    for px in frame.color.chunks_exact(4).filter(|px| *px != background) {
        colours.insert([px[0] >> 2, px[1] >> 2, px[2] >> 2]);
        sum += (f64::from(px[0]) - f64::from(px[2])) / 255.0;
        count += 1;
    }
    (
        colours.len(),
        if count == 0 {
            0.0
        } else {
            (sum / count as f64) as f32
        },
    )
}

#[test]
fn virtual_geometry_debug_views_register_and_render() {
    use helio::vg_debug_view::{VgDebugView as D, ALL};
    let Some(harness) = setup() else { return };
    let (state, _) = scene(Some(VirtualGeometryComponent::default()));
    let mut renderer = harness.renderer(&state);
    renderer.set_virtual_geometry_settings(Some(vg_settings(VirtualGeometryMode::Auto)));
    let _ = harness.until_ready(&mut renderer);
    let mailbox = renderer.editor_mailbox();

    // Every view is registered, categorised, with a unique id.
    let views = mailbox.viewport_debug_views();
    for view in ALL {
        let found = views
            .iter()
            .find(|v| v.debug_mode == view.id())
            .unwrap_or_else(|| panic!("{view:?} is missing from the editor's debug-view list"));
        assert_eq!(found.category, "Virtual Geometry", "{view:?}");
        assert_eq!(found.name, view.name());
    }
    // Our ids are registered exactly once (no other pass reuses them).
    for view in ALL {
        let count = views.iter().filter(|v| v.debug_mode == view.id()).count();
        assert_eq!(count, 1, "{view:?} is registered {count} times");
    }

    // The far camera for the LOD comparison.
    let far_distance = harness.camera.position[2] * 4.0;
    let far = Harness {
        device: harness.device.clone(),
        queue: harness.queue.clone(),
        texture: harness.texture.clone(),
        camera: EditorCameraState {
            position: [0.0, far_distance / 6.0, far_distance],
            ..harness.camera
        },
    };

    // Views with real per-pixel variation over the sphere.
    let varied = |view: D| {
        matches!(
            view,
            D::TriangleId | D::ClusterId | D::Barycentrics | D::VisNormal | D::TriangleSize
        )
    };
    let mut lod_near = 0.0;
    let mut lod_far = 0.0;
    for view in ALL {
        let scope = harness
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        mailbox.set_viewport_debug_mode(view.id());
        let near_frame = harness.frames(&mut renderer, 8);
        let far_frame = far.frames(&mut renderer, 8);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{view:?}: GPU validation error: {error:?}");
        near_frame.dump(&format!("debug_{}", view.key()));
        far_frame.dump(&format!("debug_{}_far", view.key()));
        let (near_colours, near_heat) = colour_stats(&near_frame);
        let (far_colours, far_heat) = colour_stats(&far_frame);
        println!(
            "{view:?}: near {near_colours} colours heat {near_heat:.3}, far {far_colours} colours heat {far_heat:.3}"
        );
        assert!(
            near_frame.difference(&near_frame).covered > 200,
            "{view:?}: nothing drawn"
        );
        if varied(view) {
            assert!(
                near_colours > 8,
                "{view:?}: near view is uniform ({near_colours} colours)"
            );
        }
        if view == D::LodLevel {
            (lod_near, lod_far) = (near_heat, far_heat);
        }
    }
    // The LOD view reads the cut: a distant sphere is drawn from coarser
    // clusters, which sit further up the heat ramp than the near one's.
    println!("LOD heat near {lod_near:.3} far {lod_far:.3}");
    assert!(
        lod_far > lod_near + 0.01,
        "the LOD view does not differ between near and far cameras (near {lod_near}, far {lod_far})"
    );

    // Back to lit: the sphere returns.
    mailbox.set_viewport_debug_mode(0);
    let lit = harness.frames(&mut renderer, 8);
    assert!(lit.difference(&lit).covered > 1000);
}
