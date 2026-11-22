//! Virtual geometry end to end, headless (plan
//! `.agents/plans/virtual-geometry.md`, T10): a dense mesh with
//! `virtual_geometry` on is built into a `.vgeo` in the background,
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
//! * `the_full_classic_geometry_leaves_the_gpu_and_comes_back`: once the
//!   clusters draw, only the mesh's fallback stays in the GPU mesh pools
//!   (and the Memory Explorer rows say so); turning the instance's
//!   `virtual_geometry` off brings the full geometry back.
//! * `every_mode_renders_without_validation_errors` cycles Auto,
//!   ForceNonRtFallback, ForceHwRaster and Off under a validation error scope.
//!
//! No adapter, or one without the virtual-geometry device limits: skipped.
//! Run with `--nocapture` to see the measured differences; set
//! `RENDER_ACCEPTANCE_DUMP_DIR` to write the frames as PNGs.

// The meshes' features register with the renderer as in the editor.
use installed_plugins as _;
use std::time::{Duration, Instant};

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use installed_plugins::meshes_plugin::meshes_component::components::{
    MeshAssetPath, MeshGeometryKey, StaticMeshComponent, StaticMeshMaterialSlot,
    StaticMeshMaterialSlots, VirtualGeometryComponent,
};
use installed_plugins::meshes_plugin::meshes_component::mesh_cache::MeshSection;
use installed_plugins::meshes_plugin::meshes_component::vg_settings::{
    VirtualGeometryMode, VirtualGeometrySettings,
};
use serde_json::json;
use std::collections::HashMap;
use ui_level_editor::commands::{execute_command, SceneCommand};
use ui_level_editor::scene_edit::components;
use ui_level_editor::scene_edit::{ObjectType, Transform};
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

/// The pipeline the editor ships (a renderer with none runs only its status
/// screen).
fn shipped_pipeline() -> String {
    use engine_backend::subsystems::render::helio_renderer::pipelines;
    if engine_state::get_project_path().is_none() {
        engine_state::EngineContext::new().set_global();
        engine_state::set_project_path(env!("CARGO_MANIFEST_DIR").to_string());
    }
    pipelines::migrate_pipeline_id(pipelines::LEGACY_DEFAULT_PIPELINE.to_string())
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

/// A UV sphere of `2 * rings * segments` triangles.
fn dense_sphere(
    rings: u32,
    segments: u32,
    radius: f32,
) -> (Vec<helio_mesh_api::PackedVertex>, Vec<u32>) {
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
            vertices.push(helio_mesh_api::PackedVertex::from_components(
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
            .editor_mailbox()
            .set_render_pipeline(Some(shipped_pipeline()));
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
            if installed_plugins::meshes_plugin::meshes_component::host_part::virtual_geometry_runtime(&renderer)
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
                object_type: ObjectType::Empty,
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
        let mesh = StaticMeshComponent {
            virtual_geometry: vg.is_some(),
            ..dense_mesh()
        };
        pulsar_world_registry::attach_value(&mut world, object, mesh).expect("attach mesh");
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

    // Classic: `virtual_geometry` off, so the mesh never becomes VG-active.
    let (classic_state, _) = scene(None);
    let mut classic_renderer = harness.renderer(&classic_state);
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut classic_renderer,
        Some(vg_settings(VirtualGeometryMode::Auto)),
    );
    let classic = harness.frames(&mut classic_renderer, 12);
    classic.dump("classic");
    assert!(
        classic.difference(&classic).covered > 1000,
        "the classic sphere is not drawn"
    );
    assert!(
        !installed_plugins::meshes_plugin::meshes_component::host_part::virtual_geometry_runtime(
            &classic_renderer
        )
        .expect("the viewport renderer is built")
        .any_ready()
    );

    // VG-active.
    let (state, _) = scene(Some(VirtualGeometryComponent::default()));
    let mut renderer = harness.renderer(&state);
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Auto)),
    );
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
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut root_renderer,
        Some(VirtualGeometrySettings {
            tau_pixels: 16.0,
            ..vg_settings(VirtualGeometryMode::Auto)
        }),
    );
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
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Off)),
    );
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
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut off_renderer,
        Some(vg_settings(VirtualGeometryMode::Off)),
    );
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
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Auto)),
    );
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
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Off)),
    );
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
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Auto)),
    );
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
        installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
            &mut renderer,
            Some(vg_settings(mode)),
        );
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
    use helio_vg_format::debug_view::{VgDebugView as D, ALL};
    let Some(harness) = setup() else { return };
    let (state, _) = scene(Some(VirtualGeometryComponent::default()));
    let mut renderer = harness.renderer(&state);
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Auto)),
    );
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

/// A portal object with Link ID `link`, `RADIUS * 3` across, at ground
/// position `[x, z]` turned `yaw` degrees, by the editor's commands. An
/// opening shows its far side to eyes on its local -Z side.
fn add_portal(state: &mut LevelEditorState, name: &str, [x, z]: [f32; 2], yaw: f32, link: i32) {
    let id = execute_command(
        state,
        SceneCommand::AddObject {
            data: SceneObjectData {
                id: String::new(),
                name: name.to_string(),
                object_type: ObjectType::Empty,
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
    .affected_ids[0]
        .clone();
    let placed = execute_command(
        state,
        SceneCommand::SetTransform {
            id: id.clone(),
            position: Some([f64::from(x), 0.0, f64::from(z)]),
            rotation: Some([0.0, yaw, 0.0]),
            scale: None,
        },
    );
    assert!(placed.changed, "portal transform edit was refused");
    assert!(
        execute_command(
            state,
            SceneCommand::AddComponent {
                id: id.clone(),
                class_name: "PortalComponent".into(),
                value: None,
            },
        )
        .changed
    );
    for (property, value) in [
        ("link_id", Box::new(link) as Box<dyn std::any::Any + Send>),
        ("width", Box::new(RADIUS * 3.0)),
        ("height", Box::new(RADIUS * 3.0)),
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

/// A viewport renderer of `state` with virtual geometry on.
fn vg_renderer(harness: &Harness, state: &LevelEditorState) -> HelioRenderer {
    let mut renderer = harness.renderer(state);
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Auto)),
    );
    renderer
}

/// The dense sphere (with virtual geometry, or classic) at `position`, and
/// the portals `portals` places.
fn sphere_scene(
    vg: bool,
    position: [f32; 3],
    portals: impl FnOnce(&mut LevelEditorState),
) -> LevelEditorState {
    let (mut state, id) = scene(vg.then(VirtualGeometryComponent::default));
    move_to(&mut state, &id, position);
    portals(&mut state);
    state
}

/// The pair the tests look through: the near opening at the origin facing
/// the camera (on +Z), its partner `x` metres to the side; what lies beyond
/// the partner (world -Z) shows, moved by `-x`.
fn portal_pair(state: &mut LevelEditorState, [x, z]: [f32; 2], names: [&str; 2], links: [i32; 2]) {
    add_portal(state, names[0], [x, z], 180.0, links[0]);
    add_portal(state, names[1], [x + 1000.0, z], 0.0, links[1]);
}

/// The second run's passes (`helio_vg_format::VgRun::SECONDARY`).
const THROUGH_PORTALS: [&str; 3] = [
    "VgPortalInstances",
    "VgCullEarlySecondary",
    "VgShadeSecondary",
];

/// Virtual geometry shows through a portal: a second run of the cluster
/// pipeline over the instances each live chain shows (placed where the chain
/// draws them), culled against the frame's depth after the classic
/// occurrences and clipped to the chain. The sphere sits 1 km to the side,
/// beside the far portal and out of the camera's view; the same scene with
/// unlinked portals shows none of it, and what shows covers what the classic
/// mesh shows through the same portals (the classic draw skips the
/// virtual-geometry instance, so only the second run can draw it).
#[test]
fn virtual_geometry_shows_through_a_portal() {
    let Some(harness) = setup() else { return };
    let portal_scene = |vg: bool, link: i32| {
        sphere_scene(vg, [1000.0, 0.0, -RADIUS * 2.0], |state| {
            portal_pair(state, [0.0, 0.0], ["near", "far"], [5, link]);
        })
    };
    let render = |state: &LevelEditorState, vg: bool| {
        let mut renderer = vg_renderer(&harness, state);
        let frame = if vg {
            harness.until_ready(&mut renderer)
        } else {
            harness.frames(&mut renderer, 16)
        };
        (frame, renderer.graph_members())
    };
    let (unlinked, _) = render(&portal_scene(true, 6), true);
    let (linked, members) = render(&portal_scene(true, 5), true);
    let (classic, _) = render(&portal_scene(false, 5), false);
    linked.dump("portal");
    classic.dump("portal_classic");
    let d = linked.difference(&unlinked);
    let c = classic.difference(&unlinked);
    println!("VG through a portal vs unlinked: {d:?}; classic: {c:?}");
    for pass in THROUGH_PORTALS {
        assert!(
            members.contains(&pass),
            "{pass} is not scheduled: {members:?}"
        );
    }
    assert!(
        d.depth_texels > 1000 && d.color_pixels > 1000,
        "the far side's virtual geometry did not show through the portal: {d:?}"
    );
    let ratio = d.depth_texels as f64 / c.depth_texels.max(1) as f64;
    assert!(
        (0.9..=1.1).contains(&ratio),
        "virtual geometry through the portal covers {d:?}, the classic mesh {c:?}"
    );
}

/// Virtual geometry shows through a chain two portals deep: the near pair
/// shows a second pair's near opening, which shows the sphere beyond its
/// partner. Unlinking the second pair hides the sphere; the first pair stays.
#[test]
fn virtual_geometry_shows_through_a_chain_of_two_portals() {
    let Some(harness) = setup() else { return };
    let chain_scene = |link: i32| {
        sphere_scene(true, [2000.0, 0.0, -RADIUS * 6.0], |state| {
            portal_pair(state, [0.0, 0.0], ["near", "far"], [5, 5]);
            // Beyond the far one, where the near one shows it.
            add_portal(state, "second near", [1000.0, -RADIUS * 4.0], 180.0, 7);
            add_portal(state, "second far", [2000.0, -RADIUS * 4.0], 0.0, link);
        })
    };
    let render = |state: &LevelEditorState| {
        let mut renderer = vg_renderer(&harness, state);
        harness.until_ready(&mut renderer)
    };
    let unlinked = render(&chain_scene(8));
    let linked = render(&chain_scene(7));
    linked.dump("chain");
    unlinked.dump("chain_unlinked");
    let d = linked.difference(&unlinked);
    println!("VG through a chain of two portals vs unlinked: {d:?}");
    assert!(
        d.depth_texels > 200 && d.color_pixels > 200,
        "the sphere two portals deep did not show: {d:?}"
    );
}

/// The second run is scheduled only while live chains can show virtual
/// geometry: not without portals, not through portals in a scene without
/// virtual-geometry instances, and then with both.
#[test]
fn the_portal_run_is_scheduled_only_with_chains_and_virtual_geometry() {
    let Some(harness) = setup() else { return };
    let members = |state: &LevelEditorState| {
        let mut renderer = vg_renderer(&harness, state);
        harness.until_ready(&mut renderer);
        renderer.graph_members()
    };
    let at = [1000.0, 0.0, -RADIUS * 2.0];
    for (label, state) in [
        ("no portals", sphere_scene(true, at, |_| {})),
        (
            "unlinked portals",
            sphere_scene(true, at, |state| {
                portal_pair(state, [0.0, 0.0], ["near", "far"], [5, 6])
            }),
        ),
    ] {
        let scheduled = members(&state);
        assert!(scheduled.contains(&"VgShade"), "{label}: {scheduled:?}");
        for pass in THROUGH_PORTALS {
            assert!(
                !scheduled.contains(&pass),
                "{label}: {pass} is scheduled: {scheduled:?}"
            );
        }
    }
    // Classic geometry only: the portals' own drawers, no second run.
    let classic = {
        let mut renderer = vg_renderer(
            &harness,
            &sphere_scene(false, at, |state| {
                portal_pair(state, [0.0, 0.0], ["near", "far"], [5, 5])
            }),
        );
        harness.frames(&mut renderer, 16);
        renderer.graph_members()
    };
    assert!(classic.contains(&"PortalChains"), "{classic:?}");
    for pass in THROUGH_PORTALS {
        assert!(
            !classic.contains(&pass),
            "classic only: {pass} is scheduled: {classic:?}"
        );
    }
    let scheduled = members(&sphere_scene(true, at, |state| {
        portal_pair(state, [0.0, 0.0], ["near", "far"], [5, 5])
    }));
    for pass in THROUGH_PORTALS {
        assert!(
            scheduled.contains(&pass),
            "linked: {pass} is not scheduled: {scheduled:?}"
        );
    }
}

/// Virtual geometry seen through a portal moves with the camera under TSR:
/// the second run's instances carry the chain's map of this frame and last
/// frame's (`prev_map · prev_model`), so they reproject like the same sphere
/// seen directly. The camera strafes and approaches a step a frame; where the
/// sphere shows, the last moving frame is compared with the frame settled at
/// the same camera, through the portal and, as the control, directly where
/// the portal shows it.
#[test]
fn virtual_geometry_through_a_portal_moves_without_a_tsr_smear() {
    let Some(harness) = setup() else { return };
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
    let start = harness.camera;
    let step = f64::from(RADIUS) * 4.0 * 0.02;
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
    // Renders at `camera` until the asset is ready, then settles there.
    let ready_at = |renderer: &mut HelioRenderer, camera: EditorCameraState, vg: bool| {
        let started = Instant::now();
        // The runtime exists once the renderer drew a frame.
        render(renderer, camera);
        while vg
            && !installed_plugins::meshes_plugin::meshes_component::host_part::virtual_geometry_runtime(renderer)
                .expect("the viewport renderer is built")
                .any_ready()
        {
            render(renderer, camera);
            assert!(
                started.elapsed() < Duration::from_secs(240),
                "the virtual-geometry asset never became ready"
            );
        }
        for _ in 0..16 {
            render(renderer, camera);
        }
    };
    const STEPS: usize = 12;
    let settled_at = |state: &LevelEditorState, camera: EditorCameraState, vg: bool| {
        let mut renderer = vg_renderer(&harness, state);
        ready_at(&mut renderer, camera, vg);
        harness.read_texture(&harness.texture, wgpu::TextureAspect::All)
    };
    let moved_in = |state: &LevelEditorState| {
        let mut renderer = vg_renderer(&harness, state);
        ready_at(&mut renderer, at(0), true);
        for i in 1..=STEPS {
            render(&mut renderer, at(i));
        }
        harness.read_texture(&harness.texture, wgpu::TextureAspect::All)
    };
    let differs = |a: &[u8], b: &[u8]| {
        (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > PIXEL_THRESHOLD
    };
    let lag = |name: &str, with: &LevelEditorState, without: &LevelEditorState, empty_vg: bool| {
        let moved = moved_in(with);
        let settled = settled_at(with, at(STEPS), true);
        let empty = settled_at(without, at(STEPS), empty_vg);
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
                let path = std::path::Path::new(&dir).join(format!("vg_tsr_{name}_{label}.png"));
                image::save_buffer(&path, pixels, SIZE, SIZE, image::ColorType::Rgba8)
                    .expect("write dump");
            }
        }
        (missed, shown)
    };
    // The far portal 5 km away, the sphere beyond it.
    let far_side = |link: i32| {
        sphere_scene(true, [5000.0, 0.0, -RADIUS * 2.0], |state| {
            add_portal(state, "near", [0.0, 0.0], 180.0, 5);
            add_portal(state, "far", [5000.0, 0.0], 0.0, link);
        })
    };
    let through = lag("through", &far_side(5), &far_side(6), true);
    // The control: the sphere directly where the portal shows it.
    let direct = lag(
        "direct",
        &sphere_scene(true, [0.0, 0.0, -RADIUS * 2.0], |_| {}),
        &LevelEditorState::new(),
        false,
    );
    rendering
        .set("tsr_quality", "auto")
        .expect("tsr_quality is a project setting");
    let share = |(missed, shown): (usize, usize)| missed as f64 / shown.max(1) as f64;
    println!(
        "VG PORTAL TSR motion: through the portal {} of {} pixels differ from the settled frame ({:.3}); directly {} of {} ({:.3})",
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
        "virtual geometry through the portal lags its direct motion under TSR (a smear): {through:?} against {direct:?}"
    );
}

/// A dense sphere object (with virtual geometry when `vg`) at `position`.
fn add_sphere(state: &mut LevelEditorState, position: [f32; 3], vg: bool) {
    let id = execute_command(
        state,
        SceneCommand::AddObject {
            data: SceneObjectData {
                id: String::new(),
                name: "dense sphere".to_string(),
                object_type: ObjectType::Empty,
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
    .affected_ids[0]
        .clone();
    {
        let mut world = state.scene.world_mut();
        let object = world.entity_for(&id).expect("object exists");
        let mesh = StaticMeshComponent {
            virtual_geometry: vg,
            ..dense_mesh()
        };
        pulsar_world_registry::attach_value(&mut world, object, mesh).expect("attach mesh");
        if vg {
            pulsar_world_registry::attach_value(
                &mut world,
                object,
                VirtualGeometryComponent::default(),
            )
            .expect("attach VG component");
        }
    }
    let placed = execute_command(
        state,
        SceneCommand::SetTransform {
            id,
            position: Some(position.map(f64::from)),
            rotation: None,
            scale: None,
        },
    );
    assert!(placed.changed, "transform edit was refused");
}

/// A portal object (`diag_vg_portal_cost`) with Link ID `link`, `RADIUS * 3`
/// across, at ground position `[x, z]` turned `yaw` degrees.
fn add_diag_portal(
    state: &mut LevelEditorState,
    name: &str,
    [x, z]: [f32; 2],
    yaw: f32,
    link: i32,
) {
    let id = execute_command(
        state,
        SceneCommand::AddObject {
            data: SceneObjectData {
                id: String::new(),
                name: name.to_string(),
                object_type: ObjectType::Empty,
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
    .affected_ids[0]
        .clone();
    execute_command(
        state,
        SceneCommand::SetTransform {
            id: id.clone(),
            position: Some([f64::from(x), 0.0, f64::from(z)]),
            rotation: Some([0.0, yaw, 0.0]),
            scale: None,
        },
    );
    execute_command(
        state,
        SceneCommand::AddComponent {
            id: id.clone(),
            class_name: "PortalComponent".into(),
            value: None,
        },
    );
    for (property, value) in [
        ("link_id", Box::new(link) as Box<dyn std::any::Any + Send>),
        ("width", Box::new(RADIUS * 3.0)),
        ("height", Box::new(RADIUS * 3.0)),
    ] {
        execute_command(
            state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "PortalComponent".into(),
                component_index: 0,
                prop_name: property.into(),
                value,
            },
        );
    }
}

/// GPU cost of virtual geometry through portals on a VG-heavy scene: a grid
/// of 48 dense spheres in front of the camera (drawn directly) and the same
/// grid beside a far portal, seen through one portal, and through a chain of
/// two; each against the same scene with the (last) pair unlinked. The
/// passes' own times of the frame and its views, over pipelined frames,
/// rounds interleaved. Builds in both the inline-view design and the chains
/// design (the A/B the plan doc reports).
#[test]
#[ignore = "diagnostic: prints GPU timings"]
fn diag_vg_portal_cost() {
    let Some(harness) = setup() else { return };
    let grid = |state: &mut LevelEditorState, [cx, cz]: [f32; 2]| {
        for k in 0..4 {
            for j in 0..3 {
                for i in 0..4 {
                    add_sphere(
                        state,
                        [
                            cx + (i as f32 - 1.5) * RADIUS * 0.9,
                            (j as f32 - 1.0) * RADIUS * 0.9,
                            cz - RADIUS * 2.0 - k as f32 * RADIUS * 2.0,
                        ],
                        true,
                    );
                }
            }
        }
    };
    let one = |link: i32| {
        let mut state = LevelEditorState::new();
        grid(&mut state, [RADIUS * 3.0, 0.0]);
        grid(&mut state, [1000.0, 0.0]);
        add_diag_portal(&mut state, "near", [0.0, 0.0], 180.0, 5);
        add_diag_portal(&mut state, "far", [1000.0, 0.0], 0.0, link);
        state
    };
    let chain = |link: i32| {
        let mut state = LevelEditorState::new();
        grid(&mut state, [RADIUS * 3.0, 0.0]);
        grid(&mut state, [2000.0, -RADIUS * 4.0]);
        add_diag_portal(&mut state, "near", [0.0, 0.0], 180.0, 5);
        add_diag_portal(&mut state, "far", [1000.0, 0.0], 0.0, 5);
        add_diag_portal(&mut state, "second near", [1000.0, -RADIUS * 4.0], 180.0, 7);
        add_diag_portal(&mut state, "second far", [2000.0, -RADIUS * 4.0], 0.0, link);
        state
    };
    let view = harness.texture.create_view(&Default::default());
    let measure = |state: &LevelEditorState| {
        let mut renderer = harness.renderer(state);
        installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
            &mut renderer,
            Some(vg_settings(VirtualGeometryMode::Auto)),
        );
        harness.until_ready(&mut renderer);
        harness.frames(&mut renderer, 32);
        let mut totals = Vec::new();
        for _ in 0..8 {
            for _ in 0..8 {
                renderer.queue_gizmo_mode(engine_backend::scene::GizmoType::None);
                renderer.render_frame(&harness.device, &harness.queue, &view, SIZE, SIZE, FORMAT);
            }
            harness
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            let primary: f32 = renderer.gpu_timings().map_or(0.0, |(_, passes)| {
                passes
                    .iter()
                    .filter(|(name, _)| !name.starts_with("__"))
                    .map(|(_, ms)| ms)
                    .sum()
            });
            let views: f32 = renderer
                .view_gpu_timings()
                .iter()
                .map(|(_, _, ms)| ms)
                .sum();
            totals.push(primary + views);
        }
        totals.sort_by(f32::total_cmp);
        let vg: f32 = renderer.gpu_timings().map_or(0.0, |(_, passes)| {
            passes
                .iter()
                .filter(|(name, _)| name.starts_with("Vg"))
                .map(|(_, ms)| ms)
                .sum()
        });
        if let Some((_, passes)) = renderer.gpu_timings() {
            let mut heavy: Vec<_> = passes
                .iter()
                .filter(|(name, ms)| !name.starts_with("__") && *ms > 0.1)
                .collect();
            heavy.sort_by(|a, b| b.1.total_cmp(&a.1));
            println!("DIAG VG passes: {heavy:?}");
        }
        (
            totals[totals.len() / 2],
            vg,
            renderer.view_gpu_timings().len(),
        )
    };
    for round in 0..3 {
        for (label, state) in [
            ("one unlinked", one(6)),
            ("one linked", one(5)),
            ("chain unlinked", chain(8)),
            ("chain linked", chain(7)),
        ] {
            let (total, vg, views) = measure(&state);
            println!(
                "DIAG VG round {round} {label}: {total:.3} ms (frame VG passes {vg:.3} ms, {views} views)"
            );
        }
    }
}

/// The mesh's entity (its `StaticMeshComponent`) and its full geometry's
/// content id.
fn dense_mesh_entity(state: &LevelEditorState) -> pulsar_scenedb::Entity {
    let world = state.scene.world();
    world
        .query::<&StaticMeshComponent>()
        .map(|(entity, _)| entity)
        .next()
        .expect("the dense sphere's mesh")
}

/// Bytes of `key`'s geometry interned in the scene's mesh pools.
fn resident_bytes(state: &LevelEditorState, key: u128) -> u64 {
    let world = state.scene.world();
    let store = world.gpu_mirror().expect("a GPU mirror").store();
    let id = pulsar_scenedb::handle_ledger::HandleId(key);
    let vertices = store
        .interned_var_len_pool::<helio_mesh_api::PackedVertex>(helio_mesh_api::MESH_VERTICES.key())
        .and_then(|pool| pool.resolve(id))
        .map_or(0, |(_, count)| u64::from(count) * 40);
    let indices = store
        .interned_var_len_pool::<u32>(helio_mesh_api::MESH_INDICES.key())
        .and_then(|pool| pool.resolve(id))
        .map_or(0, |(_, count)| u64::from(count) * 4);
    vertices + indices
}

/// #2048: once the clusters are drawn, a virtual-geometry mesh's full
/// classic geometry leaves the GPU and only its error-bounded fallback stays
/// (the Memory Explorer's rows say so); the frame is still the cluster
/// render. Turning the instance's `virtual_geometry` off brings the full
/// geometry back, drawn classically as before.
#[test]
fn the_full_classic_geometry_leaves_the_gpu_and_comes_back() {
    let Some(harness) = setup() else { return };
    let (classic_state, _) = scene(None);
    let mut classic_renderer = harness.renderer(&classic_state);
    let classic = harness.frames(&mut classic_renderer, 12);

    let (mut state, id) = scene(Some(VirtualGeometryComponent::default()));
    let mut renderer = harness.renderer(&state);
    installed_plugins::meshes_plugin::meshes_component::host_part::set_virtual_geometry_settings(
        &mut renderer,
        Some(vg_settings(VirtualGeometryMode::Auto)),
    );
    // The first frames attach the scene's GPU mirror and upload the mesh.
    harness.frames(&mut renderer, 2);
    let mesh = dense_mesh_entity(&state);
    let content = state
        .scene
        .world()
        .get::<StaticMeshComponent>(mesh)
        .unwrap()
        .geometry_key
        .0;
    let full_bytes = resident_bytes(&state, content);
    assert!(full_bytes > 0, "the full geometry starts on the GPU");

    harness.until_ready(&mut renderer);
    // The fallback is cut on a worker, then swapped in.
    let start = Instant::now();
    while state
        .scene
        .world()
        .get::<StaticMeshComponent>(mesh)
        .unwrap()
        .full_geometry
        .is_none()
    {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "the fallback was never swapped in"
        );
        harness.frames(&mut renderer, 2);
        std::thread::sleep(Duration::from_millis(50));
    }
    let active = harness.frames(&mut renderer, 12);
    let (fallback_key, fallback_vertices) = {
        let world = state.scene.world();
        let component = world.get::<StaticMeshComponent>(mesh).unwrap();
        assert_eq!(component.source_key().0, content);
        (component.geometry_key.0, component.vertices.len())
    };
    assert_ne!(fallback_key, content);
    let fallback_bytes = resident_bytes(&state, fallback_key);
    println!(
        "full geometry {full_bytes} bytes, fallback {fallback_bytes} bytes ({fallback_vertices} vertices)"
    );
    assert_eq!(
        resident_bytes(&state, content),
        0,
        "the full classic geometry is still on the GPU"
    );
    // What the bound saves depends on the cluster DAG; it is always less.
    assert!(fallback_bytes > 0 && fallback_bytes < full_bytes);

    // What the Memory Explorer shows for the asset.
    let runtime = installed_plugins::meshes_plugin::meshes_component::host_part::virtual_geometry_runtime(&renderer)
        .expect("the viewport renderer is built")
        .clone();
    let memory = runtime.asset_memory();
    let row = memory
        .iter()
        .find(|row| row.content == [content as u64, (content >> 64) as u64])
        .expect("the asset's memory row");
    println!("memory row: {row:?}");
    assert_eq!(row.report.classic_bytes, 0);
    assert_eq!(row.report.fallback_bytes, fallback_bytes);
    assert!(row.page_bytes > 0);

    // Still the cluster render, not the fallback's.
    let d = active.difference(&classic);
    println!("VG active (fallback resident) vs classic: {d:?}");
    assert!(
        d.color_pixels * 100 <= d.covered * 3 && d.depth_texels * 100 <= d.covered * 5,
        "{d:?}"
    );

    // The instance stops using virtual geometry: its full geometry is back.
    assert!(
        execute_command(
            &mut state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "StaticMeshComponent".into(),
                component_index: 0,
                prop_name: "virtual_geometry".into(),
                value: Box::new(false),
            },
        )
        .changed,
        "the virtual_geometry edit was refused"
    );
    let off = harness.frames(&mut renderer, 16);
    {
        let world = state.scene.world();
        let component = world.get::<StaticMeshComponent>(mesh).unwrap();
        assert_eq!(component.geometry_key.0, content);
        assert!(component.full_geometry.is_none());
    }
    assert_eq!(resident_bytes(&state, content), full_bytes);
    let o = off.difference(&classic);
    println!("instance VG off vs classic: {o:?}");
    assert!(
        o.color_pixels * 200 <= o.covered && o.depth_texels * 200 <= o.covered,
        "{o:?}"
    );
}
