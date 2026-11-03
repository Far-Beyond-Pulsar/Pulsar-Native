//! The voxel planet pass inside the default deferred graph: pipeline and
//! attachment compatibility, residency settling, source removal and resize.
use helio_components::{AtmosphereComponent, LightComponent};
use std::sync::{Arc, Mutex};

use glam::Vec3;
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    RendererBuilder, RendererConfig,
};
use helio_core::RenderPass;
use helio_feature_api::{Feature, FeatureRegistrar, NodeDescriptor, Slot};
use helio_pass_voxel_planet::chunks::pass::ChunkPass;
use helio_pass_voxel_planet::engine::{PlanetFrame, VOXEL_FRAME};
use helio_pass_voxel_planet::{Planet, PlanetRecipe};
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

/// The engine's features over Helio's: the voxel terrain's.
fn voxel_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_voxel_planet::VoxelFeature)
}

struct FinalResourceConsumer {
    expected: [u32; 2],
    observed: Arc<Mutex<Vec<[u32; 2]>>>,
}

/// Adds a [`FinalResourceConsumer`] after the output, at the graph's
/// internal size.
struct FinalConsumerFeature(Arc<Mutex<Vec<[u32; 2]>>>);

impl Feature for FinalConsumerFeature {
    fn id(&self) -> &'static str {
        "test.final_consumer"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        let observed = self.0.clone();
        registrar.node(
            NodeDescriptor::new(
                "test.final_consumer",
                "Final consumer",
                "Test",
                "Reads the frame after the output.",
                move |build| {
                    let (width, height) = build.env.internal_size();
                    build.graph.add_pass(Box::new(FinalResourceConsumer {
                        expected: [width, height],
                        observed: observed.clone(),
                    }));
                    Ok(())
                },
            )
            .with_generated_pins()
            .in_slot(Slot::Output, 0),
        );
    }
}
impl helio_core::RenderPass for FinalResourceConsumer {
    fn name(&self) -> &'static str {
        "FinalResourceConsumer"
    }
    fn reads(&self) -> &'static [&'static str] {
        &["pre_aa"]
    }
    // A resize keeps the pass: it learns the new size here.
    fn on_resize(&mut self, _device: &wgpu::Device, size: helio_core::RenderSize) {
        self.expected = size.internal;
    }
    fn execute(&mut self, ctx: &mut helio_core::PassContext) -> helio_core::Result<()> {
        let texture = ctx.resource_pool.get_texture("pre_aa").unwrap();
        let extent = [texture.width(), texture.height()];
        assert_eq!(extent, self.expected);
        assert!(ctx
            .registry
            .texture_view(helio_core::ResourceKey::new("pre_aa"))
            .is_some());
        self.observed.lock().unwrap().push(extent);
        Ok(())
    }
}

#[test]
fn chunk_pass_builds_settles_and_resizes_in_the_deferred_graph() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: voxel default graph");
            return;
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: required_wgpu_features(adapter.features()),
                required_limits: required_wgpu_limits(adapter.limits()),
                experimental_features: required_experimental_features(adapter.features()),
                ..Default::default()
            })
            .await
            .unwrap();
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let gpu_context = EngineGpuContext::new(Arc::clone(&device), Arc::clone(&queue));
        let mut gpu_store = SceneGpuStore::new(
            &gpu_context,
            SceneGpuConfig {
                classes: Vec::new(),
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        );
        helio_components::AtmosphereComponent::register_gpu_columns_growable(
            &mut gpu_store,
            4,
            &device,
        );
        helio_components::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 16, &device);
        helio_components::MaterialComponent::register_gpu_columns_growable(
            &mut gpu_store,
            16,
            &device,
        );
        helio_components::StaticObjectComponent::register_gpu_columns_growable(
            &mut gpu_store,
            16,
            &device,
        );
        helio_components::LightComponent::register_gpu_columns_growable(
            &mut gpu_store,
            16,
            &device,
        );
        let mirror = GpuMirrorHandle::new(Arc::new(gpu_store), Arc::clone(&queue));

        let mut config = RendererConfig::new(640, 360, wgpu::TextureFormat::Rgba8Unorm);
        config
            .features
            .edit::<helio_pass_foliage_place::FoliageSettings>()
            .enabled = false;
        let config = config.with_feature(helio_pass_voxel_planet::VoxelSettings::default());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let nodes = helio_presets::graphs::registry_for(
            &helio_presets::features::default_features()
                .extended(&voxel_features())
                .with(FinalConsumerFeature(observed.clone())),
        );
        let mut renderer = RendererBuilder::new(config.clone(), mirror)
            .with_ambient([0.5, 0.5, 0.6], 1.0)
            .with_external_device()
            .with_graph(Preset::Deferred.pipeline_with(Arc::new(nodes)))
            .build(
                Arc::clone(&device),
                Arc::clone(&queue),
                640,
                360,
                config.surface_format,
            );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Voxel graph smoke target"),
            size: wgpu::Extent3d {
                width: 640,
                height: 360,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());

        // The camera at the planet eye; the frame's origin follows it.
        let planet = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
        let eye = planet.surface_point(glam::DVec3::new(0.2, 1.0, 0.3), 1.7);
        let up = eye.normalize().as_vec3();
        let forward = (up.any_orthonormal_vector() - up * 0.2).normalize();
        let camera = Camera::perspective_look_to(
            eye,
            forward,
            up,
            std::f32::consts::FRAC_PI_4,
            640.0 / 360.0,
            0.05,
            40_000_000.0,
        );

        // Without a planet frame the pass is inert.
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.render(&camera, &view).unwrap();
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let error = validation.pop().await;
        assert!(error.is_none(), "empty graph GPU validation: {error:?}");
        assert!(!renderer.find_pass::<ChunkPass>().unwrap().has_world());

        // A host's first eye can be anywhere (an editor camera starts at the
        // planet's centre): nothing to stream there.
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        publish(&mut renderer, Some(PlanetFrame::single(planet.clone(), up)));
        let centre = Camera::perspective_look_to(
            glam::DVec3::new(8.0, 6.0, 12.0),
            forward,
            up,
            std::f32::consts::FRAC_PI_4,
            640.0 / 360.0,
            0.05,
            40_000_000.0,
        );
        for _ in 0..600 {
            renderer.render(&centre, &view).unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            if !renderer.find_pass::<ChunkPass>().unwrap().needs_frame() {
                break;
            }
        }
        assert!(
            !renderer.find_pass::<ChunkPass>().unwrap().needs_frame(),
            "the centre never settled"
        );

        // Moved to the surface, it streams the planet around the eye until
        // settled: the plan for the old eye does not settle the new one
        // (hosts stop rendering once settled).
        let mut frames = 0;
        let started = std::time::Instant::now();
        loop {
            renderer.render(&camera, &view).unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let pass = renderer.find_pass::<ChunkPass>().unwrap();
            if pass.has_world() {
                frames += 1;
            }
            assert!(
                started.elapsed().as_secs() < 300,
                "residency did not settle"
            );
            if !pass.needs_frame() || frames >= 600 {
                break;
            }
        }
        let error = validation.pop().await;
        assert!(error.is_none(), "active graph GPU validation: {error:?}");
        let pass = renderer.find_pass::<ChunkPass>().unwrap();
        let stats = pass.stats();
        assert!(
            frames < 600,
            "residency still streaming after {frames} frames: {stats:?}"
        );
        assert!(stats.resident_chunks > 1000, "{stats:?}");
        assert!(pass.settled());
        eprintln!(
            "VOXEL_GRAPH_SETTLED frames={frames} resident={}",
            stats.resident_chunks
        );
        // A still view stays settled: hosts render until it is (the editor
        // viewport goes idle only after its settling frames pass with no
        // request). Periodic counter readbacks used to request frames
        // forever.
        for still in 0..120 {
            renderer.render(&camera, &view).unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            assert!(
                !renderer.find_pass::<ChunkPass>().unwrap().needs_frame(),
                "a settled still view asked for frame {still}"
            );
        }

        // Every debug view the graph advertises (the editor's menu) has a
        // mode of its own and draws without validation errors.
        let views = renderer.available_debug_views();
        let mut modes: Vec<u32> = views.iter().map(|v| v.debug_mode).collect();
        modes.sort_unstable();
        let unique = modes.len();
        modes.dedup();
        assert_eq!(
            modes.len(),
            unique,
            "debug modes shared: {:?}",
            views
                .iter()
                .map(|v| (v.debug_mode, v.name))
                .collect::<Vec<_>>()
        );
        for v in &views {
            let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
            renderer.set_debug_mode(v.debug_mode);
            for _ in 0..2 {
                renderer.render(&camera, &view).unwrap();
            }
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let error = validation.pop().await;
            assert!(
                error.is_none(),
                "debug view {} ({}): {error:?}",
                v.debug_mode,
                v.name
            );
        }
        renderer.set_debug_mode(0);
        eprintln!(
            "DEBUG_VIEWS {}",
            views
                .iter()
                .map(|v| format!("{}={}", v.debug_mode, v.name))
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Removing the volumes drops the planet's chunks.
        publish(&mut renderer, None);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer.render(&camera, &view).unwrap();
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert!(validation.pop().await.is_none());
        let pass = renderer.find_pass::<ChunkPass>().unwrap();
        assert!(!pass.has_world());
        assert!(!pass.needs_frame());
        assert!(observed
            .lock()
            .unwrap()
            .contains(&[config.internal_width(), config.internal_height()]));

        // A resize resizes the graph in place; downstream passes see the new
        // size.
        publish(&mut renderer, Some(PlanetFrame::single(planet, up)));
        renderer.set_render_size(320, 180);
        let resized = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("final consumer resize"),
            size: wgpu::Extent3d {
                width: 320,
                height: 180,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        renderer
            .render(&camera, &resized.create_view(&Default::default()))
            .unwrap();
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert!(validation.pop().await.is_none());
        let resized_config = RendererConfig {
            width: 320,
            height: 180,
            ..config
        };
        assert_eq!(
            observed.lock().unwrap().last(),
            Some(&[
                resized_config.internal_width(),
                resized_config.internal_height()
            ])
        );
        assert!(renderer.find_pass::<ChunkPass>().unwrap().has_world());
    });
}

/// Publish `frame` as the scene's voxel volumes (none: no volumes).
fn publish(renderer: &mut helio::Renderer, frame: Option<PlanetFrame>) {
    match frame {
        Some(frame) => renderer.set_scene_payload(&VOXEL_FRAME, Arc::new(frame)),
        None => renderer.set_scene_payloads(helio_core::ScenePayloads::default()),
    }
}

struct Editor {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    renderer: helio::Renderer,
    target: wgpu::Texture,
    scene: pulsar_scenedb::SceneDb,
    sun: Option<pulsar_scenedb::Entity>,
    air: Option<pulsar_scenedb::Entity>,
}

fn editor(width: u32, height: u32) -> Option<Editor> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: editor overlays");
        return None;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: required_wgpu_features(adapter.features()),
        required_limits: required_wgpu_limits(adapter.limits()),
        experimental_features: required_experimental_features(adapter.features()),
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) = (Arc::new(device), Arc::new(queue));
    let gpu_context = EngineGpuContext::new(Arc::clone(&device), Arc::clone(&queue));
    let mut gpu_store = SceneGpuStore::new(
        &gpu_context,
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    helio_components::AtmosphereComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4,
        &device,
    );
    helio_components::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    helio_components::MaterialComponent::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    helio_components::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        16,
        &device,
    );
    helio_components::LightComponent::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    let mirror = GpuMirrorHandle::new(Arc::new(gpu_store), Arc::clone(&queue));
    let mut scene = pulsar_scenedb::SceneDb::new();
    scene.world.attach_gpu_mirror(mirror.clone());
    let mut config = RendererConfig::new(width, height, wgpu::TextureFormat::Rgba8Unorm);
    config
        .features
        .edit::<helio_pass_foliage_place::FoliageSettings>()
        .enabled = false;
    let config = config.with_feature(helio_pass_voxel_planet::VoxelSettings::default());
    let mut renderer = RendererBuilder::new(config.clone(), mirror)
        .with_ambient([0.5, 0.5, 0.6], 1.0)
        .with_external_device()
        .with_graph(Preset::Deferred.with_features(&voxel_features()))
        .build(
            Arc::clone(&device),
            Arc::clone(&queue),
            width,
            height,
            config.surface_format,
        );
    renderer.set_editor_mode(true);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("editor overlay target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    Some(Editor {
        device,
        queue,
        renderer,
        target,
        scene,
        sun: None,
        air: None,
    })
}

impl Editor {
    /// A directional sun shining from `towards`, and the scene's atmosphere
    /// row (`None` removes it).
    fn set_sky(&mut self, towards: Vec3, air: Option<helio_components::AtmosphereComponent>) {
        let sun = *self.sun.get_or_insert_with(|| self.scene.world.spawn());
        self.scene.world.insert(
            sun,
            LightComponent::from(helio_pass_forward_lit::LightRow::from(helio::GpuLight {
                position_range: [0.0, 0.0, 0.0, f32::MAX],
                direction_outer: [-towards.x, -towards.y, -towards.z, 0.0],
                color_intensity: [1.0, 1.0, 1.0, 3.0],
                shadow_index: u32::MAX,
                light_type: helio::LightType::Directional as u32,
                ..Default::default()
            })),
        );
        let entity = *self.air.get_or_insert_with(|| self.scene.world.spawn());
        match air {
            Some(air) => self.scene.world.insert(entity, air),
            None => {
                self.scene
                    .world
                    .remove::<helio_components::AtmosphereComponent>(entity);
            }
        }
        self.scene.world.flush_gpu_mirror(&self.queue);
    }

    /// Publish `frame` as the scene's voxel volumes (none: no volumes).
    fn show(&mut self, frame: Option<PlanetFrame>) {
        publish(&mut self.renderer, frame);
    }

    fn render(&mut self, camera: &Camera) {
        self.renderer
            .render(camera, &self.target.create_view(&Default::default()))
            .unwrap();
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
    }

    /// Fractions of pixels in the grid's axis colours (red x axis, green z
    /// axis) and in its neutral grey line colour.
    fn rgba(&self) -> Vec<[u8; 4]> {
        let size = self.target.size();
        let row = (size.width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size.height),
                },
            },
            size,
        );
        self.queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        (0..size.height as usize)
            .flat_map(|y| {
                data[y * row as usize..][..size.width as usize * 4]
                    .chunks_exact(4)
                    .map(|px| <[u8; 4]>::try_from(px).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn overlay_fractions(&self) -> (f64, f64) {
        let pixels = self.rgba();
        let (mut axis, mut grey) = (0usize, 0usize);
        for px in &pixels {
            let [r, g, b] = [i32::from(px[0]), i32::from(px[1]), i32::from(px[2])];
            axis += usize::from((r > g + 40 && r > b + 30) || (g > r + 60 && g > b + 60));
            grey += usize::from((r - g).abs() < 12 && (g - b).abs() < 16 && r < 170);
        }
        (
            axis as f64 / pixels.len() as f64,
            grey as f64 / pixels.len() as f64,
        )
    }
}

/// Editor overlays in a camera-relative frame (world origin at the eye) must
/// stay in world space. The grid used to put its y = 0 plane through the eye:
/// pressed against voxel ground with a tiny near plane, every pixel hit it
/// next to the x axis and the view turned the axis' red.
#[test]
fn editor_overlays_stay_in_world_space_in_camera_relative_frames() {
    let Some(mut editor) = editor(320, 180) else {
        return;
    };
    let planet = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
    let ground = planet.surface_point(glam::DVec3::new(0.2, 1.0, 0.3), 0.0);
    let up = ground.normalize();
    let forward = up.any_orthonormal_vector().as_vec3();
    editor.show(Some(PlanetFrame::single(planet.clone(), up.as_vec3())));
    let validation = editor
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    // An editor camera held against the ground: a few centimetres of
    // clearance, the smallest near plane and a planetary far plane.
    for step in 0..120 {
        let eye = ground + up * (0.02 + 0.004 * f64::from(step % 10));
        let camera = Camera::perspective_look_to(
            eye,
            forward,
            up.as_vec3(),
            std::f32::consts::FRAC_PI_4,
            16.0 / 9.0,
            0.05,
            40_000_000.0,
        );
        editor.render(&camera);
        if step % 20 == 19 {
            let (axis, _) = editor.overlay_fractions();
            assert!(
                axis < 0.001,
                "step {step}: {:.1}% of the view in grid axis colours",
                axis * 100.0
            );
        }
    }
    assert!(pollster::block_on(validation.pop()).is_none());

    // Near the world's origin the grid shows: above an empty scene, looking
    // down at the origin, the lines and both axes appear.
    editor.show(None);
    let eye = Vec3::new(3.0, 6.0, 3.0);
    let camera = Camera::perspective_look_at(
        eye,
        Vec3::ZERO,
        Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        16.0 / 9.0,
        0.1,
        1000.0,
    );
    for _ in 0..4 {
        editor.render(&camera);
    }
    let (axis, grey) = editor.overlay_fractions();
    assert!(
        axis > 0.001 && grey > 0.01,
        "grid missing: axis {axis:.4}, lines {grey:.4}"
    );
}

const EARTH_RADIUS: f64 = 6_360_000.0;
/// Earth's atmosphere preset: 100 km of air.
const AIR_THICKNESS: f64 = 100_000.0;

fn earth_air() -> helio_components::AtmosphereComponent {
    AtmosphereComponent::from(
        helio_pass_sky::AtmosphereRow::earth().around_planet([0.0; 3], EARTH_RADIUS),
    )
}

fn mean_rgb(pixels: &[[u8; 4]]) -> [f64; 3] {
    std::array::from_fn(|c| {
        pixels.iter().map(|p| f64::from(p[c])).sum::<f64>() / pixels.len() as f64
    })
}

fn capture(editor: &Editor, name: &str, pixels: &[[u8; 4]]) {
    if let Ok(output) = std::env::var("HELIO_ATMOSPHERE_CAPTURE") {
        std::fs::create_dir_all(&output).unwrap();
        let size = editor.target.size();
        let bytes: Vec<_> = pixels.iter().flat_map(|p| p.iter().copied()).collect();
        image::save_buffer(
            std::path::Path::new(&output).join(format!("{name}.png")),
            &bytes,
            size.width,
            size.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}

/// The same local camera and sun at two poles must see the same sky, and a
/// resize must not reset it. The atmosphere is a scene row; the sun is the
/// scene's directional light; the frame's origin follows the eye.
#[test]
fn atmosphere_follows_the_world_eye_and_sun_through_resize() {
    let Some(mut editor) = editor(256, 144) else {
        return;
    };
    editor.renderer.set_editor_mode(false);
    let validation = editor
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let altitude = EARTH_RADIUS + 3_000.0;
    let cases = [
        (
            glam::DVec3::Y * altitude,
            Vec3::new(0.6, 0.8, 0.0),
            Vec3::Y,
            Vec3::new(0.4, 0.8, 0.2),
        ),
        (
            glam::DVec3::X * altitude,
            Vec3::new(0.8, -0.6, 0.0),
            Vec3::X,
            Vec3::new(0.8, -0.4, 0.2),
        ),
    ];
    let mut means = Vec::new();
    for (eye, forward, up, sun) in cases {
        editor.set_sky(sun.normalize(), Some(earth_air()));
        let camera = Camera::perspective_look_to(
            eye,
            forward,
            up,
            std::f32::consts::FRAC_PI_4,
            16.0 / 9.0,
            0.05,
            40_000_000.0,
        );
        for _ in 0..8 {
            editor.render(&camera);
        }
        let pixels = editor.rgba();
        capture(
            &editor,
            if up == Vec3::Y {
                "rotated_y"
            } else {
                "rotated_x"
            },
            &pixels,
        );
        let mean = mean_rgb(&pixels);
        eprintln!("ATMOSPHERE_ROTATED mean {mean:?}");
        means.push(mean);
    }
    assert!(
        means[0][2] > means[0][0] + 10.0,
        "day sky must be blue: {means:?}"
    );
    for c in 0..3 {
        assert!(
            (means[0][c] - means[1][c]).abs() < 6.0,
            "rotated sky mismatch: {means:?}"
        );
    }
    editor.renderer.set_render_size(320, 180);
    editor.target = editor.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("resized atmosphere"),
        size: wgpu::Extent3d {
            width: 320,
            height: 180,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let (eye, forward, up, _) = cases[1];
    let camera = Camera::perspective_look_to(
        eye,
        forward,
        up,
        std::f32::consts::FRAC_PI_4,
        16.0 / 9.0,
        0.05,
        40_000_000.0,
    );
    for _ in 0..8 {
        editor.render(&camera);
    }
    let mean = mean_rgb(&editor.rgba());
    for c in 0..3 {
        assert!(
            (mean[c] - means[1][c]).abs() < 6.0,
            "sky reset during resize: {mean:?} vs {:?}",
            means[1]
        );
    }
    assert!(pollster::block_on(validation.pop()).is_none());
}

/// The thin orbital limb must stay circular at any world orientation: no
/// light outside the shell, no dark sectors in it.
#[test]
fn orbital_atmosphere_has_no_wedges_outside_the_shell() {
    const SIZE: u32 = 513;
    let Some(mut editor) = editor(SIZE, SIZE) else {
        return;
    };
    editor.renderer.set_editor_mode(false);
    editor.renderer.set_tsr_quality(None);
    editor.renderer.set_jitter_enabled(false);
    let radius = EARTH_RADIUS;
    for (case, distance_scale, up) in [
        ("north", 1.5, Vec3::Y),
        ("equator", 1.5, Vec3::X),
        ("south", 1.5, -Vec3::Y),
        ("opposite_equator", 1.5, -Vec3::X),
        ("tilted", 3.0, Vec3::new(0.3, 0.7, -0.4).normalize()),
        ("distant", 16.0, Vec3::new(-0.4, 0.2, 0.8).normalize()),
    ] {
        let distance = radius * distance_scale;
        let eye = up.as_dvec3() * distance;
        let outer_angle = ((radius + AIR_THICKNESS) / distance).asin();
        let half_fov_tan = outer_angle.tan() * 1.25;
        editor.set_sky(up, Some(earth_air()));
        let camera = Camera::perspective_look_to(
            eye,
            -up,
            up.any_orthonormal_vector(),
            (2.0 * half_fov_tan.atan()) as f32,
            1.0,
            0.05,
            400_000_000.0,
        );
        for _ in 0..4 {
            editor.render(&camera);
        }
        let pixels = editor.rgba();
        capture(&editor, case, &pixels);
        let mut outside = 0;
        let mut outside_bright = 0;
        let mut sectors = [[0usize; 2]; 24];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let nx = 2.0 * (f64::from(x) + 0.5) / f64::from(SIZE) - 1.0;
                let ny = 2.0 * (f64::from(y) + 0.5) / f64::from(SIZE) - 1.0;
                let tan_theta = nx.hypot(ny) * half_fov_tan;
                let impact = distance * tan_theta / (1.0 + tan_theta * tan_theta).sqrt();
                let pixel = pixels[(y * SIZE + x) as usize];
                // FXAA may spread the edge over two pixels.
                if nx.hypot(ny) > outer_angle.tan() / half_fov_tan + 4.0 / f64::from(SIZE) {
                    outside += 1;
                    outside_bright += usize::from(pixel[2] > 4);
                }
                if impact > radius + 3_000.0 && impact < radius + 18_000.0 {
                    let azimuth = (ny.atan2(nx) + std::f64::consts::PI) / std::f64::consts::TAU;
                    let sector = ((azimuth * 24.0) as usize).min(23);
                    sectors[sector][0] += 1;
                    sectors[sector][1] += usize::from(pixel[2] > 20);
                }
            }
        }
        eprintln!("ORBITAL_ATMOSPHERE {case}: outside={outside_bright}/{outside}, limb sectors={sectors:?}");
        assert!(outside > 10_000);
        assert_eq!(
            outside_bright, 0,
            "{case}: light leaks outside the atmosphere"
        );
        for (sector, [count, bright]) in sectors.into_iter().enumerate() {
            assert!(
                count > 3 && bright * 10 >= count * 9,
                "{case}: dark limb sector {sector}: {bright}/{count}"
            );
        }
    }
}

/// From orbit with the sun on the horizon of the pole below, the limb is lit
/// on the sun's side and dark opposite, and a small sweep of the sun through
/// the terminator changes it smoothly.
#[test]
fn terminator_is_smooth_and_the_night_side_is_dark() {
    const SIZE: u32 = 513;
    let Some(mut editor) = editor(SIZE, SIZE) else {
        return;
    };
    editor.renderer.set_editor_mode(false);
    editor.renderer.set_tsr_quality(None);
    editor.renderer.set_jitter_enabled(false);
    let radius = EARTH_RADIUS;
    let distance = radius * 1.5;
    let half_fov_tan = (((radius + AIR_THICKNESS) / distance).asin()).tan() * 1.25;
    let camera = Camera::perspective_look_to(
        glam::DVec3::Y * distance,
        -Vec3::Y,
        Vec3::Z,
        (2.0 * half_fov_tan.atan()) as f32,
        1.0,
        0.05,
        40_000_000.0,
    );
    let mut means = Vec::new();
    for (case, elevation) in [
        ("twilight_before", -0.005),
        ("twilight", 0.0),
        ("twilight_after", 0.005),
    ] {
        editor.set_sky(
            Vec3::new(1.0, elevation, 0.0).normalize(),
            Some(earth_air()),
        );
        for _ in 0..4 {
            editor.render(&camera);
        }
        let pixels = editor.rgba();
        capture(&editor, case, &pixels);
        let mut sides = [Vec::new(), Vec::new()];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let nx = 2.0 * (f64::from(x) + 0.5) / f64::from(SIZE) - 1.0;
                let ny = 2.0 * (f64::from(y) + 0.5) / f64::from(SIZE) - 1.0;
                let tan_theta = nx.hypot(ny) * half_fov_tan;
                let impact = distance * tan_theta / (1.0 + tan_theta * tan_theta).sqrt();
                if impact > radius + 3_000.0 && impact < radius + 18_000.0 && nx.abs() > 0.3 {
                    sides[usize::from(nx > 0.0)]
                        .push(f64::from(pixels[(y * SIZE + x) as usize][2]));
                }
            }
        }
        let side_means = sides.map(|v| v.iter().sum::<f64>() / v.len() as f64);
        eprintln!("TWILIGHT_ATMOSPHERE {case}: blue={side_means:?}");
        let (lit, dark) = (
            side_means[0].max(side_means[1]),
            side_means[0].min(side_means[1]),
        );
        assert!(
            lit > dark + 20.0,
            "{case}: no day side on the limb: {side_means:?}"
        );
        assert!(dark < 8.0, "{case}: the night side glows: {side_means:?}");
        means.push(side_means);
    }
    for pair in means.windows(2) {
        for c in 0..2 {
            assert!(
                (pair[0][c] - pair[1][c]).abs() < 10.0,
                "abrupt twilight switch: {means:?}"
            );
        }
    }
}

/// Not only planets: the default placement puts flat ground at the world's
/// origin. A camera standing on it sees a blue sky above, a brighter horizon
/// and darker ground below it, and nothing once the atmosphere row is gone.
#[test]
fn atmosphere_over_flat_ground_without_a_planet() {
    let Some(mut editor) = editor(256, 144) else {
        return;
    };
    editor.renderer.set_editor_mode(false);
    editor.renderer.set_tsr_quality(None);
    editor.renderer.set_jitter_enabled(false);
    let validation = editor
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let sun = Vec3::new(0.3, 0.6, 0.2).normalize();
    editor.set_sky(
        sun,
        Some(AtmosphereComponent::from(
            helio_pass_sky::AtmosphereRow::earth(),
        )),
    );
    let eye = Vec3::new(0.0, 2.0, 0.0);
    let up_camera = Camera::perspective_look_at(
        eye,
        eye + Vec3::new(-0.3, 1.0, 0.1),
        Vec3::Z,
        std::f32::consts::FRAC_PI_4,
        16.0 / 9.0,
        0.1,
        10_000.0,
    );
    for _ in 0..4 {
        editor.render(&up_camera);
    }
    let zenith = mean_rgb(&editor.rgba());
    let level_camera = Camera::perspective_look_at(
        eye,
        Vec3::new(-10.0, 2.0, 3.0),
        Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        16.0 / 9.0,
        0.1,
        10_000.0,
    );
    for _ in 0..4 {
        editor.render(&level_camera);
    }
    let pixels = editor.rgba();
    capture(&editor, "flat_horizon", &pixels);
    let width = editor.target.size().width as usize;
    let rows = pixels.len() / width;
    let row_mean = |row: usize| mean_rgb(&pixels[row * width..(row + 1) * width]);
    let (above, below) = (row_mean(rows / 2 - 6), row_mean(rows / 2 + 12));
    let sum = |c: [f64; 3]| c[0] + c[1] + c[2];
    eprintln!("FLAT_ATMOSPHERE zenith {zenith:?} above horizon {above:?} below {below:?}");
    assert!(
        zenith[2] > zenith[0] + 15.0,
        "zenith must be blue: {zenith:?}"
    );
    assert!(
        sum(above) > sum(zenith),
        "horizon must be brighter than the zenith"
    );
    assert!(sum(above) > sum(below), "ground must be below the horizon");
    editor.set_sky(sun, None);
    for _ in 0..4 {
        editor.render(&up_camera);
    }
    let removed = mean_rgb(&editor.rgba());
    assert!(
        removed.iter().all(|&c| c < 2.0),
        "atmosphere left behind after removal: {removed:?}"
    );
    assert!(pollster::block_on(validation.pop()).is_none());
}

/// Inspector edits get only one rendered frame before an idle viewport.
/// Discarding colour history must show that edit without rebuilding chunks.
#[test]
fn appearance_edit_is_visible_in_one_frame_without_rebuilding_residency() {
    let Some(mut editor) = editor(128, 72) else {
        return;
    };
    editor.renderer.set_editor_mode(false);
    editor
        .renderer
        .set_tsr_quality(Some(helio_pass_tsr::TsrQuality::Native));
    editor.renderer.set_jitter_enabled(false);
    editor.renderer.set_ambient([1.0; 3], 1.0);
    let planet = Arc::new(
        Planet::new(PlanetRecipe {
            shape: helio_pass_voxel_planet::grid::Shape::Plane,
            plane_size_m: 1024.0,
            terrain: helio_pass_voxel_planet::layers::TerrainLayers::flat().source(7),
            ..Default::default()
        })
        .unwrap(),
    );
    let eye = glam::DVec3::Y * 20.0;
    // The volume as its owner shows it: with `appearance` (none: the
    // generator's own).
    let look = |appearance| {
        let mut frame = PlanetFrame::single(planet.clone(), Vec3::Y);
        frame.volumes[0].appearance = appearance;
        Some(frame)
    };
    editor.show(look(None));
    let camera = Camera::perspective_look_to(
        eye,
        -Vec3::Y,
        Vec3::Z,
        std::f32::consts::FRAC_PI_4,
        16.0 / 9.0,
        0.05,
        1000.0,
    );
    let started = std::time::Instant::now();
    let mut frames = 0;
    while frames < 2000 && started.elapsed().as_secs() < 300 {
        editor.render(&camera);
        let pass = editor.renderer.find_pass::<ChunkPass>().unwrap();
        if pass.settled() {
            break;
        }
        if pass.has_world() {
            frames += 1;
        }
    }
    assert!(editor.renderer.find_pass::<ChunkPass>().unwrap().settled());
    for _ in 0..16 {
        editor.render(&camera);
    }
    let before = editor.rgba();
    let chunks = editor
        .renderer
        .find_pass::<ChunkPass>()
        .unwrap()
        .stats()
        .resident_chunks;
    let mean = |pixels: &[[u8; 4]], channel| {
        pixels.iter().map(|p| f64::from(p[channel])).sum::<f64>() / pixels.len() as f64
    };
    let mut appearance = helio_pass_voxel_planet::engine::TerrainAppearance::default();
    appearance.materials[helio_pass_voxel_planet::terrain::material::GRASS as usize].patches =
        Some([[0.85, 0.03, 0.03]; 3]);
    appearance.detail = [0.0; 4];
    editor.show(look(Some(appearance)));
    editor
        .renderer
        .find_pass_mut::<helio_pass_tsr::TsrPass>()
        .unwrap()
        .reset_history();
    editor.render(&camera);
    let after = editor.rgba();
    // A new look asks an on-demand host for frames while history settles.
    assert!(editor.renderer.needs_frame(), "a new look asks for frames");
    eprintln!(
        "appearance before {:?}, after {:?}",
        [mean(&before, 0), mean(&before, 1)],
        [mean(&after, 0), mean(&after, 1)]
    );
    assert!(mean(&after, 0) > mean(&after, 1) + 20.0);
    assert!(mean(&after, 0) > mean(&before, 0) + 20.0);
    assert_eq!(
        editor
            .renderer
            .find_pass::<ChunkPass>()
            .unwrap()
            .stats()
            .resident_chunks,
        chunks
    );
    // Settled, the same look published again is no change.
    for _ in 0..64 {
        if !editor.renderer.needs_frame() {
            break;
        }
        editor.render(&camera);
    }
    assert!(!editor.renderer.needs_frame(), "the new look never settled");
    editor.show(look(Some(appearance)));
    editor.render(&camera);
    assert!(!editor.renderer.needs_frame(), "the same look is no change");
    editor.show(look(None));
    editor
        .renderer
        .find_pass_mut::<helio_pass_tsr::TsrPass>()
        .unwrap()
        .reset_history();
    editor.render(&camera);
    let restored = editor.rgba();
    assert!(mean(&restored, 1) > mean(&restored, 0));
    editor
        .renderer
        .set_tsr_quality(Some(helio_pass_tsr::TsrQuality::Quality));
    editor.render(&camera);
    assert_eq!(
        editor.renderer.renderer_config().tsr_quality,
        Some(helio_pass_tsr::TsrQuality::Quality)
    );
    assert!(editor
        .renderer
        .find_pass::<helio_pass_tsr::TsrPass>()
        .is_some());
    editor.renderer.set_tsr_quality(None);
    editor.render(&camera);
    assert_eq!(editor.renderer.renderer_config().tsr_quality, None);
    assert!(editor
        .renderer
        .find_pass::<helio_pass_tsr::TsrPass>()
        .is_none());
}

/// Terrain under an atmosphere, from standing height to high above it, under
/// a high sun and a low one in view: no black frame (a non-finite value
/// spreading through temporal history, or a missing sky).
#[test]
fn terrain_under_an_atmosphere_is_lit_at_every_height() {
    let Some(mut editor) = editor(320, 180) else {
        return;
    };
    editor.renderer.set_editor_mode(false);
    let planet = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
    let radius = planet.grid().radius();
    let validation = editor
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let dir = glam::DVec3::new(0.2, 1.0, 0.3);
    let up = planet.surface_point(dir, 0.0).normalize();
    // High, and low ahead of the camera (the sun's disk in view).
    for (case, sun) in [
        (
            "high",
            (up + up.any_orthonormal_vector() * 0.6)
                .normalize()
                .as_vec3(),
        ),
        (
            "low",
            (up.any_orthonormal_vector() + up * 0.08)
                .normalize()
                .as_vec3(),
        ),
    ] {
        editor.set_sky(
            sun,
            Some(AtmosphereComponent::from(
                helio_pass_sky::AtmosphereRow::earth().around_planet([0.0; 3], radius),
            )),
        );
        for height in [2.0, 25.0, 300.0, 20_000.0] {
            let eye = planet.surface_point(dir, height);
            editor.show(Some(PlanetFrame::single(planet.clone(), sun)));
            let forward = (up.any_orthonormal_vector() - up * 0.2)
                .normalize()
                .as_vec3();
            let camera = Camera::perspective_look_to(
                eye,
                forward,
                up.as_vec3(),
                std::f32::consts::FRAC_PI_4,
                16.0 / 9.0,
                0.05,
                40_000_000.0,
            );
            let started = std::time::Instant::now();
            loop {
                editor.render(&camera);
                let settled = editor.renderer.find_pass::<ChunkPass>().unwrap().settled();
                if settled || started.elapsed().as_secs() > 120 {
                    break;
                }
            }
            for _ in 0..8 {
                editor.render(&camera);
            }
            let pixels = editor.rgba();
            capture(&editor, &format!("terrain_{case}_{height}"), &pixels);
            let mean = mean_rgb(&pixels);
            eprintln!("TERRAIN_ATMOSPHERE {case} sun, {height} m: mean {mean:?}");
            // Backlit ground under a low sun is dark, but the sky around the sun
            // is not; under a high sun the lit ground fills the view.
            let brightest = pixels
                .iter()
                .map(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]))
                .max()
                .unwrap();
            assert!(
                brightest > 90,
                "{case} sun, {height} m: the view is black: {mean:?}"
            );
            if case == "high" {
                assert!(
                    mean.iter().sum::<f64>() > 60.0,
                    "{height} m: sunlit terrain too dark: {mean:?}"
                );
            }
        }
    }
    assert!(pollster::block_on(validation.pop()).is_none());
}

/// A host's save asks the pass for the edited chunks through the frame
/// (a keep request in `VolumeRequests`): the pass asks for frames until it
/// answers into the volume's inbox, once, with the planet shown and its
/// world's kept chunks. Given back with the loaded
/// planet (`PlanetFrame::kept`), a new world restores the edited chunks by
/// copy instead of generating them; chunks of another history are refused.
#[test]
fn saved_edited_chunks_come_back_through_the_frame() {
    use helio_pass_voxel_planet::chunks::KeptChunks;
    use helio_pass_voxel_planet::engine::{
        VolumeAnswer, VolumeAnswers, VolumeFrame, VolumeRequests,
    };
    use helio_pass_voxel_planet::{Brush, BrushOp, BrushShape};
    let Some(mut editor) = editor(160, 90) else {
        return;
    };
    editor.renderer.set_editor_mode(false);
    let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
    let dir = glam::DVec3::new(0.2, 1.0, 0.3);
    let ground = planet.surface_point(dir, 0.0);
    let up = ground.normalize();
    let side = up.any_orthonormal_vector();
    let fwd = up.cross(side);
    for k in 0..300 {
        let (a, b) = (f64::from(k % 20) - 9.5, f64::from(k / 20) - 7.0);
        let center = ground + side * (a * 2.0) + fwd * (b * 2.0 + 20.0) + up * 2.0;
        planet
            .apply(Brush {
                center: center.to_array(),
                radius: 4.0,
                shape: BrushShape::Sphere,
                op: BrushOp::Remove,
                material: 0,
                height: 0.0,
            })
            .unwrap();
    }
    let planet = Arc::new(planet);
    let eye = ground + up * 6.0;
    let forward = (fwd - up * 0.3).normalize().as_vec3();
    let camera = Camera::perspective_look_to(
        eye,
        forward,
        up.as_vec3(),
        std::f32::consts::FRAC_PI_4,
        16.0 / 9.0,
        0.05,
        40_000_000.0,
    );
    let frame = |planet: &Arc<Planet>, kept: Option<Arc<KeptChunks>>, requests| {
        Some(PlanetFrame {
            volumes: vec![VolumeFrame {
                kept,
                requests,
                ..VolumeFrame::new(helio_pass_voxel_planet::VoxelVolume::new(
                    0,
                    planet.clone(),
                    helio_pass_voxel_planet::VolumeTransform::IDENTITY,
                ))
            }],
            sun: up.as_vec3(),
        })
    };
    // Settled, it counts the edited chunks restored and generated.
    let settle = |editor: &mut Editor| {
        let started = std::time::Instant::now();
        let (mut restored, mut generated) = (0, 0);
        loop {
            editor.render(&camera);
            let pass = editor.renderer.find_pass::<ChunkPass>().unwrap();
            restored += pass.stats().restored_chunks;
            generated += pass.stats().generated_edited;
            if pass.has_world() && !pass.needs_frame() {
                return (restored, generated);
            }
            assert!(
                started.elapsed().as_secs() < 300,
                "residency did not settle"
            );
        }
    };
    let validation = editor
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let answers = Arc::new(VolumeAnswers::default());
    let asking = |keep| VolumeRequests {
        keep,
        answers: Some(answers.clone()),
        ..Default::default()
    };
    editor.show(frame(&planet, None, asking(None)));
    let (_, edited) = settle(&mut editor);
    assert!(edited > 50, "{edited} edited chunks generated");

    editor.show(frame(&planet, None, asking(Some(1))));
    let mut frames = 0;
    let (shown, kept) = loop {
        editor.render(&camera);
        frames += 1;
        let answer = answers.take().into_iter().find_map(|a| match a {
            VolumeAnswer::Kept { id, planet, chunks } => Some((id, planet, chunks)),
            _ => None,
        });
        if let Some((id, shown, kept)) = answer {
            assert_eq!(id, 1, "the keep request answered");
            break (shown, kept);
        }
        assert!(
            editor
                .renderer
                .find_pass::<ChunkPass>()
                .unwrap()
                .needs_frame(),
            "a keep request waits for frames"
        );
        assert!(frames < 200, "no kept chunks after {frames} frames");
    };
    assert!(Arc::ptr_eq(&shown, &planet));
    // Answered once: the request still published asks nothing more.
    for _ in 0..8 {
        editor.render(&camera);
    }
    assert!(answers.take().is_empty(), "a keep request answered twice");
    assert!(kept.len() > 50, "{} chunks kept", kept.len());
    let bytes = kept.to_bytes();

    // Loaded: a planet from the edit snapshot, a new world with the chunks.
    let loaded =
        Arc::new(Planet::from_snapshot(PlanetRecipe::default(), &planet.snapshot()).unwrap());
    let chunks = Arc::new(KeptChunks::from_bytes(&bytes).unwrap());
    editor.show(None);
    editor.render(&camera);
    editor.show(frame(
        &loaded,
        Some(chunks.clone()),
        VolumeRequests::default(),
    ));
    let (restored, generated) = settle(&mut editor);
    let pass = editor.renderer.find_pass::<ChunkPass>().unwrap();
    assert_eq!(pass.kept_loaded(), Some(&Ok(kept.len())));
    eprintln!("KEPT_THROUGH_FRAME {} chunks ({} KiB) after {frames} frames; loaded: {restored} restored, {generated} generated ({edited} generated before)", kept.len(), bytes.len() / 1024);
    assert!(
        restored > 50 && generated * 4 < restored,
        "{restored} restored, {generated} generated"
    );

    // Another history: refused, generated as before.
    let mut other = (*loaded).clone();
    other
        .apply(Brush {
            center: ground.to_array(),
            radius: 1.0,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        })
        .unwrap();
    editor.show(None);
    editor.render(&camera);
    editor.show(frame(
        &Arc::new(other),
        Some(chunks),
        VolumeRequests::default(),
    ));
    editor.render(&camera);
    assert!(editor
        .renderer
        .find_pass::<ChunkPass>()
        .unwrap()
        .kept_loaded()
        .is_some_and(|r| r.is_err()));
    assert!(pollster::block_on(validation.pop()).is_none());
}
