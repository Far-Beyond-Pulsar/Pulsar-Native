//! Virtual geometry Tier 3 (dense overlap) end to end, headless: a far crowd of
//! instanced noisy spheres whose sub-pixel triangles overlap so heavily that
//! the raster classifies tiles as dense, resolved by the depth-peel backend
//! (`ForceNonRtFallback`) and, where the device supports ray queries, by the
//! ray-traced backend (`Auto`).
//!
//! * the dense-tile mask becomes non-empty (debug view 106),
//! * NonRT renders; RT renders and agrees with NonRT within a tolerance
//!   (the RT path traces proxy LODs, so it is approximate by design),
//! * `ForceNonRtFallback` never touches the RT machinery (no tier, no proxy
//!   builds, no BLAS/TLAS), also after a session that used RT,
//! * the TLAS is rebuilt only when the instance set changes, not per frame,
//! * no GPU validation errors in any mode.
//!
//! No adapter, or one without the virtual-geometry limits: skipped. No
//! ray-query device: the RT half is skipped (Auto must then equal NonRT).
//! `RENDER_ACCEPTANCE_DUMP_DIR` writes the frames as PNGs; `--nocapture`
//! prints the measured numbers.

use std::time::{Duration, Instant};

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
use helio_component::components::{
    MeshAssetPath, MeshGeometryKey, StaticMeshComponent, StaticMeshMaterialSlot,
    StaticMeshMaterialSlots, VirtualGeometryComponent,
};
use helio_component::mesh_cache::MeshSection;
use pulsar_settings::project::virtual_geometry::{VirtualGeometryMode, VirtualGeometrySettings};
use std::collections::HashMap;
use ui_level_editor::commands::{execute_command, SceneCommand};
use ui_level_editor::scene_edit::{MeshType, ObjectType, Transform};
use ui_level_editor::{LevelEditorState, SceneObjectData};

const SIZE: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const PIXEL_THRESHOLD: u32 = 48;
const RADIUS: f32 = 40.0;
const CROWD: usize = 40;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }
}

/// A UV sphere whose vertices are pushed radially by white noise (25 % of the
/// radius): the simplifier cannot remove detail whose deviation exceeds the
/// error threshold, so far-away instances keep thousands of tiny triangles.
fn noisy_sphere(rings: u32, segments: u32) -> (Vec<helio_core::PackedVertex>, Vec<u32>) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let theta = std::f32::consts::PI * ring as f32 / rings as f32;
        let mut first = None;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let normal = glam::Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            let tangent = glam::Vec3::new(-phi.sin(), 0.0, phi.cos());
            // The seam column repeats the first column's radius so the mesh stays closed.
            let jitter = if segment == segments {
                first.unwrap()
            } else {
                rng.range(-0.25, 0.25)
            };
            if segment == 0 {
                first = Some(jitter);
            }
            // Poles stay put (one vertex per pole position).
            let r = if ring == 0 || ring == rings {
                RADIUS
            } else {
                RADIUS * (1.0 + jitter)
            };
            vertices.push(helio_core::PackedVertex::from_components(
                (normal * r).into(),
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

fn crowd_mesh() -> StaticMeshComponent {
    let (vertices, indices) = noisy_sphere(110, 150);
    let content: u128 = 0x5eed_c0de_0000_0000_0000_0000_d3a5_e002;
    StaticMeshComponent {
        mesh_asset: MeshAssetPath::generated(content),
        geometry_key: MeshGeometryKey(content),
        material_slots: StaticMeshMaterialSlots {
            slots: vec![StaticMeshMaterialSlot {
                name: "crowd".into(),
                surface_override: Some(helio_component::mesh_cache::ImportedSurfaceMaterial {
                    base_color: [0.9, 0.55, 0.2, 1.0],
                    roughness: 0.7,
                    metallic: 0.0,
                    emissive: [0.9, 0.55, 0.2],
                    emissive_intensity: 3.0,
                    alpha: 1.0,
                }),
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
        bounds_local: [0.0, 0.0, 0.0, RADIUS * 1.25],
        ..Default::default()
    }
}

/// Instance placements: a deep, narrow column straight ahead of the camera so
/// the spheres overlap on screen.
fn placements() -> Vec<([f64; 3], f32)> {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    (0..CROWD)
        .map(|_| {
            let z = -rng.range(220.0, 560.0);
            (
                [
                    rng.range(-9.0, 9.0) as f64,
                    rng.range(-7.0, 7.0) as f64,
                    z as f64,
                ],
                rng.range(0.18, 0.3),
            )
        })
        .collect()
}

fn scene() -> LevelEditorState {
    let mut state = LevelEditorState::new();
    for (i, (position, scale)) in placements().into_iter().enumerate() {
        let id = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: format!("crowd {i}"),
                    object_type: ObjectType::Mesh(MeshType::Custom),
                    transform: Transform {
                        position,
                        rotation: [0.0; 3],
                        scale: [scale; 3],
                    },
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
        let mut world = state.scene.world_mut();
        let object = world.entity_for(&id).expect("object exists");
        pulsar_world_registry::attach_value(&mut world, object, crowd_mesh()).expect("attach mesh");
        pulsar_world_registry::attach_value(
            &mut world,
            object,
            VirtualGeometryComponent::default(),
        )
        .expect("attach VG");
    }
    state
}

/// A device that keeps ray queries when the adapter can run them.
fn device(want_rt: bool) -> Option<(wgpu::Device, wgpu::Queue, bool)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let features = if want_rt {
        helio::usable_adapter_features(&adapter)
    } else {
        adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()?;
    let rt = device
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
        && device.limits().max_tlas_instance_count > 0
        && device.limits().max_blas_geometry_count > 0;
    Some((device, queue, rt))
}

#[derive(Clone)]
struct Frame {
    color: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct Difference {
    color_pixels: usize,
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
        let background = &self.color[..4];
        let covered = self
            .color
            .chunks_exact(4)
            .filter(|px| *px != background)
            .count();
        Difference {
            color_pixels,
            covered,
        }
    }

    /// Pixels painted with the dense-tile-mask view's orange.
    fn dense_mask_pixels(&self) -> usize {
        self.color
            .chunks_exact(4)
            .filter(|p| p[0] > 220 && (100..160).contains(&p[1]) && p[2] < 50)
            .count()
    }

    fn dump(&self, name: &str) {
        let Some(dir) = std::env::var_os("RENDER_ACCEPTANCE_DUMP_DIR") else {
            return;
        };
        let path = std::path::Path::new(&dir).join(format!("vg_dense_{name}.png"));
        image::save_buffer(&path, &self.color, SIZE, SIZE, image::ColorType::Rgba8)
            .expect("write dump");
    }
}

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    camera: EditorCameraState,
    rt: bool,
}

impl Harness {
    fn new(device: wgpu::Device, queue: wgpu::Queue, rt: bool) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vg-dense-acceptance-target"),
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
        let camera = EditorCameraState {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        Self {
            device,
            queue,
            texture,
            camera,
            rt,
        }
    }

    fn frames(&self, renderer: &mut HelioRenderer, frames: usize) -> Frame {
        let view = self.texture.create_view(&Default::default());
        let mut encoded = false;
        for frame in 0..frames {
            // A still camera lets the renderer skip frames; the nudge ends at the base pose.
            let yaw = self.camera.yaw + if frame % 2 == 0 { 1e-4 } else { 0.0 };
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
        Frame {
            color: self.read_texture(),
        }
    }

    fn read_texture(&self) -> Vec<u8> {
        let row = SIZE * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vg-dense-acceptance-readback"),
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
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let mapped = slice.get_mapped_range().expect("mapped range");
        let data = mapped
            .chunks_exact(padded as usize)
            .flat_map(|l| l[..row as usize].iter().copied())
            .collect();
        drop(mapped);
        buffer.unmap();
        data
    }

    fn until_ready(&self, renderer: &mut HelioRenderer) {
        let start = Instant::now();
        loop {
            let _ = self.frames(renderer, 4);
            if renderer
                .virtual_geometry_runtime()
                .expect("the viewport renderer is built")
                .any_ready()
            {
                let _ = self.frames(renderer, 12);
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(300),
                "the virtual-geometry asset never became ready"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Renders until `done(stats)` holds, at most `timeout`.
    fn until(
        &self,
        renderer: &mut HelioRenderer,
        timeout: Duration,
        what: &str,
        done: impl Fn(u64) -> bool,
    ) {
        let start = Instant::now();
        loop {
            let _ = self.frames(renderer, 6);
            let stats = renderer
                .virtual_geometry_runtime()
                .expect("the viewport renderer is built")
                .tier3_stats();
            if done(stats.frames_rt) {
                return;
            }
            assert!(
                start.elapsed() < timeout,
                "timed out waiting for {what}: {stats:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn settings(mode: VirtualGeometryMode) -> VirtualGeometrySettings {
    VirtualGeometrySettings {
        mode,
        tier3: true,
        ..Default::default()
    }
}

fn setup(want_rt: bool) -> Option<Harness> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
    let Some((device, queue, rt)) = device(want_rt) else {
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
    Some(Harness::new(device, queue, rt))
}

fn validated<T>(harness: &Harness, label: &str, f: impl FnOnce() -> T) -> T {
    let scope = harness
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let out = f();
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{label}: GPU validation error: {error:?}");
    out
}

#[test]
fn dense_crowd_resolves_with_both_tier3_backends() {
    let Some(harness) = setup(true) else { return };
    let state = scene();
    let mut renderer = HelioRenderer::new(state.scene.shared_scene());
    renderer.set_editor_camera_state(harness.camera);

    // ── NonRT: the baseline, and the proof that tiles go dense ───────────
    renderer.set_virtual_geometry_settings(Some(settings(VirtualGeometryMode::ForceNonRtFallback)));
    harness.until_ready(&mut renderer);
    let runtime = renderer
        .virtual_geometry_runtime()
        .expect("the viewport renderer is built")
        .clone();
    let nonrt = validated(&harness, "ForceNonRtFallback", || {
        harness.frames(&mut renderer, 12)
    });
    nonrt.dump("nonrt");
    let covered = nonrt.difference(&nonrt).covered;
    println!("NonRT: covered pixels {covered}");
    assert!(covered > 300, "the crowd is not drawn ({covered} pixels)");

    let mailbox = renderer.editor_mailbox();
    // Streaming refines the crowd over many frames; tiles go dense once enough
    // fine pages are resident, so wait for the mask rather than a frame count.
    mailbox.set_viewport_debug_mode(helio::vg_debug_view::VgDebugView::DenseMask.id());
    let start = Instant::now();
    let mask_view = loop {
        let frame = harness.frames(&mut renderer, 3);
        if frame.dense_mask_pixels() >= 256 || start.elapsed() > Duration::from_secs(180) {
            break frame;
        }
    };
    mask_view.dump("dense_mask");
    let dense_pixels = mask_view.dense_mask_pixels();
    println!(
        "dense tile mask: {dense_pixels} pixels ({} tiles) after {:?}",
        dense_pixels / 256,
        start.elapsed()
    );
    mailbox.set_viewport_debug_mode(0);
    assert!(
        dense_pixels >= 256,
        "the crowd never made a tile dense ({dense_pixels} mask pixels)"
    );
    // The baseline for the RT comparison: the settled crowd.
    let nonrt = validated(&harness, "ForceNonRtFallback (settled)", || {
        harness.frames(&mut renderer, 12)
    });
    nonrt.dump("nonrt_settled");
    let covered = nonrt.difference(&nonrt).covered;

    let stats = runtime.tier3_stats();
    println!("after ForceNonRtFallback: {stats:?}");
    assert!(stats.frames_nonrt > 0, "{stats:?}");
    assert_eq!(stats.frames_rt, 0, "ForceNonRtFallback ran the RT backend");
    assert_eq!(
        stats.rt_tier_frames, 0,
        "ForceNonRtFallback touched the RT machinery"
    );
    assert_eq!(
        stats.blas_builds + stats.tlas_rebuilds + stats.proxy_jobs_started,
        0,
        "{stats:?}"
    );

    // ── Auto ─────────────────────────────────────────────────────────────
    renderer.set_virtual_geometry_settings(Some(settings(VirtualGeometryMode::Auto)));
    if !harness.rt {
        let frame = validated(&harness, "Auto (no RT)", || {
            harness.frames(&mut renderer, 12)
        });
        let stats = runtime.tier3_stats();
        println!("SKIP RT: no ray-query device. Auto: {stats:?}");
        assert_eq!(stats.frames_rt, 0);
        assert_eq!(stats.rt_tier_frames, 0);
        let d = frame.difference(&nonrt);
        assert!(
            d.color_pixels * 100 <= d.covered,
            "Auto without RT must equal NonRT: {d:?}"
        );
        return;
    }

    validated(&harness, "Auto (RT warm-up)", || {
        harness.until(
            &mut renderer,
            Duration::from_secs(240),
            "the first ray-traced frame",
            |frames_rt| frames_rt > 0,
        );
    });
    // Settle: proxies resident, TLAS stable.
    let rt_frame = validated(&harness, "Auto (RT)", || harness.frames(&mut renderer, 16));
    rt_frame.dump("rt");
    let before = runtime.tier3_stats();
    let rt_frame_2 = harness.frames(&mut renderer, 24);
    let after = runtime.tier3_stats();
    println!("Auto/RT: {after:?}");
    assert!(after.rt_hardware);
    assert!(
        after.frames_rt > before.frames_rt,
        "RT did not keep running while the scene was static: {before:?} -> {after:?}"
    );
    assert!(
        after.proxy_assets >= 1 && after.proxy_bytes > 0,
        "{after:?}"
    );
    assert!(after.tlas_instances >= 2, "{after:?}");
    assert!(
        after.blas_builds >= 1 && after.tlas_rebuilds >= 1,
        "{after:?}"
    );
    // Rebuild only on change: a static scene reuses the TLAS every frame.
    assert!(
        after.tlas_rebuilds - before.tlas_rebuilds <= 1,
        "the TLAS was rebuilt {} times in 24 static frames",
        after.tlas_rebuilds - before.tlas_rebuilds
    );
    assert!(
        after.tlas_reuses > before.tlas_reuses,
        "{before:?} -> {after:?}"
    );
    assert_eq!(
        after.blas_builds, before.blas_builds,
        "BLASes were rebuilt in a static scene"
    );

    // RT vs NonRT: same crowd within a tolerance (the proxy is a cut of the
    // DAG at the error ladder's nearest level, so edges and noisy detail differ).
    let d = rt_frame_2.difference(&nonrt);
    println!("RT vs NonRT: {d:?}");
    let covered_rt = rt_frame_2.difference(&rt_frame_2).covered;
    assert!(
        covered_rt * 100 >= covered * 85 && covered_rt * 100 <= covered * 115,
        "coverage {covered_rt} vs {covered}"
    );
    assert!(
        d.color_pixels * 100 <= d.covered * 25,
        "RT differs from NonRT on too many pixels: {d:?}"
    );

    // The debug tier view paints ray-traced pixels magenta.
    mailbox.set_viewport_debug_mode(helio::vg_debug_view::VgDebugView::RasterTier.id());
    let tier_view = harness.frames(&mut renderer, 9);
    tier_view.dump("tier");
    mailbox.set_viewport_debug_mode(0);
    let magenta = tier_view
        .color
        .chunks_exact(4)
        .filter(|p| p[0] > 200 && p[1] < 80 && p[2] > 180)
        .count();
    println!("tier view: {magenta} ray-traced (magenta) pixels");
    // Informational: the resolve merges the RT key with Tier 1/2 by depth (hardware
    // wins ties), so on a crowd the coarse proxy rarely wins a pixel outright;
    // that RT ran is asserted through the counters above.
    assert!(after.dense_tiles_processed > 0, "{after:?}");

    // ── ForceNonRtFallback after RT: the RT machinery goes quiet ─────────
    renderer.set_virtual_geometry_settings(Some(settings(VirtualGeometryMode::ForceNonRtFallback)));
    let _ = harness.frames(&mut renderer, 4);
    let quiet_from = runtime.tier3_stats();
    let forced = validated(&harness, "ForceNonRtFallback after RT", || {
        harness.frames(&mut renderer, 24)
    });
    let quiet_to = runtime.tier3_stats();
    assert_eq!(
        quiet_to.frames_rt, quiet_from.frames_rt,
        "RT ran under ForceNonRtFallback"
    );
    assert_eq!(
        quiet_to.rt_tier_frames, quiet_from.rt_tier_frames,
        "the RT tier ran under ForceNonRtFallback"
    );
    assert_eq!(quiet_to.blas_builds, quiet_from.blas_builds);
    assert_eq!(quiet_to.tlas_rebuilds, quiet_from.tlas_rebuilds);
    assert_eq!(quiet_to.proxy_jobs_started, quiet_from.proxy_jobs_started);
    assert!(quiet_to.frames_nonrt > quiet_from.frames_nonrt);
    let back = forced.difference(&nonrt);
    assert!(
        back.color_pixels * 100 <= back.covered * 3,
        "NonRT changed between sessions: {back:?}"
    );

    // ── Off and ForceHwRaster never run Tier 3 at all ────────────────────
    for mode in [VirtualGeometryMode::ForceHwRaster, VirtualGeometryMode::Off] {
        renderer.set_virtual_geometry_settings(Some(settings(mode)));
        let from = runtime.tier3_stats();
        validated(&harness, mode.as_str(), || {
            let _ = harness.frames(&mut renderer, 8);
        });
        let to = runtime.tier3_stats();
        assert_eq!(
            (to.frames_rt, to.frames_nonrt),
            (from.frames_rt, from.frames_nonrt),
            "{mode:?} ran Tier 3"
        );
    }
}

/// A device without ray queries (the feature stripped, as on the adapters the
/// compiler-crash filter covers): Auto has no ray tracer, so the depth peel runs
/// only where Tier 2 is the tiled no-int64 fallback; on a 64-bit-atomics GPU the
/// dense tiles stay on Tier 2 (measured cheaper) and Tier 3 does nothing.
#[test]
fn auto_without_ray_queries_is_the_fallback() {
    let Some(harness) = setup(false) else { return };
    let state = scene();
    let mut renderer = HelioRenderer::new(state.scene.shared_scene());
    renderer.set_editor_camera_state(harness.camera);
    renderer.set_virtual_geometry_settings(Some(settings(VirtualGeometryMode::Auto)));
    harness.until_ready(&mut renderer);
    let runtime = renderer
        .virtual_geometry_runtime()
        .expect("the viewport renderer is built")
        .clone();
    let frame = validated(&harness, "Auto without RT", || {
        harness.frames(&mut renderer, 16)
    });
    let stats = runtime.tier3_stats();
    println!("Auto without ray queries: {stats:?}");
    assert!(frame.difference(&frame).covered > 300);
    assert!(!stats.rt_hardware);
    assert_eq!(stats.frames_rt, 0);
    assert_eq!(stats.rt_tier_frames, 0);
    assert_eq!(
        stats.frames_nonrt > 0,
        stats.tier2_tile_fallback,
        "{stats:?}"
    );
}

/// Regression for the dense-mask feedback loop. The cull routes sub-pixel
/// clusters that overlap a dense tile to Tier 3, which starves the raster
/// counters that classify the tile; the mask used to collapse and come back on
/// alternate frames (4,0,4,0 tiles; ragged lit coverage). On a static camera
/// the raster's own classification must now be stable, the lit crowd must not
/// flicker, and the NonRT peel must not drop a single cluster-list entry.
#[test]
fn dense_mask_is_stable_on_a_static_scene_and_the_peel_drops_nothing() {
    let Some(harness) = setup(false) else { return };
    let state = scene();
    let mut renderer = HelioRenderer::new(state.scene.shared_scene());
    renderer.set_editor_camera_state(harness.camera);
    renderer.set_virtual_geometry_settings(Some(settings(VirtualGeometryMode::ForceNonRtFallback)));
    harness.until_ready(&mut renderer);
    let runtime = renderer
        .virtual_geometry_runtime()
        .expect("the viewport renderer is built")
        .clone();
    let mailbox = renderer.editor_mailbox();

    // Streaming refines the crowd; wait for the dense mask (raster classification).
    mailbox.set_viewport_debug_mode(helio::vg_debug_view::VgDebugView::DenseMask.id());
    let start = Instant::now();
    loop {
        let f = harness.frames(&mut renderer, 3);
        if f.dense_mask_pixels() >= 256 || start.elapsed() > Duration::from_secs(180) {
            break;
        }
    }
    // Let the streaming settle so the cut stops changing.
    let _ = harness.frames(&mut renderer, 60);
    let mut tiles = Vec::new();
    for _ in 0..30 {
        tiles.push(harness.frames(&mut renderer, 1).dense_mask_pixels() / 256);
    }
    println!("dense tiles per frame (static camera): {tiles:?}");
    let (lo, hi) = (*tiles.iter().min().unwrap(), *tiles.iter().max().unwrap());
    assert!(hi > 0, "the crowd never made a tile dense");
    assert!(
        (hi - lo) * 10 < hi,
        "the dense mask is not stable: {tiles:?}"
    );

    mailbox.set_viewport_debug_mode(0);
    let mut covered = Vec::new();
    let mut overflow = 0;
    let mut previous: Option<Frame> = None;
    let mut worst_diff = 0;
    for _ in 0..16 {
        let f = harness.frames(&mut renderer, 1);
        covered.push(f.difference(&f).covered);
        if let Some(p) = &previous {
            worst_diff = worst_diff.max(p.difference(&f).color_pixels);
        }
        previous = Some(f);
        overflow += runtime.tier3_stats().bin_overflow;
    }
    println!("lit coverage per frame: {covered:?}, worst frame-to-frame diff {worst_diff} px, overflow {overflow}");
    let (clo, chi) = (
        *covered.iter().min().unwrap(),
        *covered.iter().max().unwrap(),
    );
    assert!(
        chi > 300 && (chi - clo) * 100 <= chi,
        "lit coverage flickers: {covered:?}"
    );
    assert!(
        worst_diff * 100 <= chi,
        "consecutive lit frames differ by {worst_diff} px of {chi}"
    );
    assert_eq!(
        overflow,
        0,
        "the NonRT peel dropped cluster-list entries: {:?}",
        runtime.tier3_stats()
    );
}

/// Debug view 103 reflects what Tier 3 used: pixels of resolved dense tiles are
/// orange (or magenta for RT), not the blue of the cull bin they came from.
#[test]
fn tier_view_paints_resolved_dense_tiles() {
    let Some(harness) = setup(false) else { return };
    let state = scene();
    let mut renderer = HelioRenderer::new(state.scene.shared_scene());
    renderer.set_editor_camera_state(harness.camera);
    renderer.set_virtual_geometry_settings(Some(settings(VirtualGeometryMode::ForceNonRtFallback)));
    harness.until_ready(&mut renderer);
    let mailbox = renderer.editor_mailbox();
    mailbox.set_viewport_debug_mode(helio::vg_debug_view::VgDebugView::DenseMask.id());
    let start = Instant::now();
    loop {
        let f = harness.frames(&mut renderer, 3);
        if f.dense_mask_pixels() >= 256 || start.elapsed() > Duration::from_secs(180) {
            break;
        }
    }
    mailbox.set_viewport_debug_mode(helio::vg_debug_view::VgDebugView::RasterTier.id());
    let tier = harness.frames(&mut renderer, 6);
    tier.dump("tier_nonrt");
    mailbox.set_viewport_debug_mode(0);
    let orange = tier.dense_mask_pixels();
    println!("tier view: {orange} dense (orange) pixels");
    assert!(
        orange > 0,
        "the tier view never shows the resolved dense tiles"
    );
}
