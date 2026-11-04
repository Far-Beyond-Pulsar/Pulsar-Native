//! The atmosphere's clouds in the default deferred graph: they appear in
//! their band of altitudes and nowhere else, their shadow darkens the
//! ground under them along the sun, a still view holds still, and what they
//! cost.
use helio_components::LightComponent;
use std::sync::Arc;

use glam::Vec3;
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    RendererBuilder, RendererConfig,
};
use helio_pass_sky::host_rows::AtmosphereComponent;
use helio_pass_sky::{CloudLayer, PlanetWeather};
use helio_pass_voxel_planet::chunks::pass::ChunkPass;
use helio_pass_voxel_planet::engine::{PlanetFrame, VOXEL_FRAME};
use helio_pass_voxel_planet::{Planet, PlanetRecipe};
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

/// The engine's features over Helio's: the voxel terrain's and the media
/// kinds' (foliage, water, fog, post-process volumes), as the engine adds them.
fn voxel_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_voxel_planet::VoxelFeature)
        .with(helio_pass_foliage_gbuffer::FoliageFeature)
        .with(helio_pass_water_sim::WaterFeature)
        .with(helio_pass_volumetric_fog::FogFeature)
        .with(helio_pass_post_volumes::PostVolumesFeature)
        .with(helio_pass_sky::SkyFeature)
        .with(helio_pass_gi::GiFeature)
}

struct View {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    renderer: helio::Renderer,
    target: wgpu::Texture,
    scene: pulsar_scenedb::SceneDb,
    sun: pulsar_scenedb::Entity,
    air: pulsar_scenedb::Entity,
    /// Whether the view has a voxel ground (it shows what is published
    /// under `VOXEL_FRAME`).
    ground: bool,
}

/// A renderer of `width` x `height` without temporal anti-aliasing or
/// jitter (the clouds' own temporal filter is what is tested); with a voxel
/// pass when `ground`.
fn view(width: u32, height: u32, ground: bool) -> Option<View> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    })) else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: clouds");
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
    AtmosphereComponent::register_gpu_columns_growable(&mut gpu_store, 4, &device);
    helio_mesh_host::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    helio_components::MaterialComponent::register_gpu_columns_growable(&mut gpu_store, 16, &device);
    helio_mesh_host::StaticObjectComponent::register_gpu_columns_growable(
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
    if ground {
        config = config.with_feature(helio_pass_voxel_planet::VoxelSettings::default());
    }
    let mut renderer = RendererBuilder::new(config.clone(), mirror)
        .with_feature(helio::AmbientLight {
            color: [0.0; 3],
            intensity: 0.0,
        })
        .with_external_device()
        .with_graph(helio_mesh_host::presets::with(
            helio_presets::Preset::Deferred,
            &voxel_features(),
        ))
        .build(
            Arc::clone(&device),
            Arc::clone(&queue),
            width,
            height,
            config.surface_format,
        );
    renderer.set_editor_mode(false);
    renderer.set_tsr_quality(None);
    renderer.set_jitter_enabled(false);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("clouds target"),
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
    let (sun, air) = (scene.world.spawn(), scene.world.spawn());
    Some(View {
        device,
        queue,
        renderer,
        target,
        scene,
        sun,
        air,
        ground,
    })
}

impl View {
    /// A sun shining from `towards` and the atmosphere `air`.
    fn set_sky(&mut self, towards: Vec3, air: AtmosphereComponent) {
        self.scene.world.insert(
            self.sun,
            LightComponent::from(helio_pass_light_cull::LightRow::from(helio::GpuLight {
                position_range: [0.0, 0.0, 0.0, f32::MAX],
                direction_outer: [-towards.x, -towards.y, -towards.z, 0.0],
                color_intensity: [1.0, 1.0, 1.0, 3.0],
                shadow_index: u32::MAX,
                light_type: helio::LightType::Directional as u32,
                ..Default::default()
            })),
        );
        self.scene.world.insert(self.air, air);
        self.scene.world.flush_gpu_mirror(&self.queue);
    }

    fn render(&mut self, camera: &Camera) {
        self.renderer
            .render(camera, &self.target.create_view(&Default::default()))
            .unwrap();
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
    }

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

    /// `frames` frames of `camera`, then the image.
    fn settle(&mut self, camera: &Camera, frames: usize) -> Vec<[u8; 4]> {
        for _ in 0..frames {
            self.render(camera);
        }
        self.rgba()
    }

    fn capture(&self, name: &str, pixels: &[[u8; 4]]) {
        if let Ok(output) = std::env::var("HELIO_CLOUDS_CAPTURE") {
            std::fs::create_dir_all(&output).unwrap();
            let size = self.target.size();
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
}

fn luminance(p: [u8; 4]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn mean_abs_difference(a: &[[u8; 4]], b: &[[u8; 4]]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (luminance(*a) - luminance(*b)).abs())
        .sum::<f64>()
        / a.len() as f64
}

/// A camera at `eye` (world metres) looking along `forward`.
fn camera(eye: Vec3, forward: Vec3, fov_deg: f32, aspect: f32) -> Camera {
    let up = if forward.normalize().y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Camera::perspective_look_at(
        eye,
        eye + forward,
        up,
        fov_deg.to_radians(),
        aspect,
        0.1,
        1.0e7,
    )
}

fn band() -> CloudLayer {
    CloudLayer {
        bottom_km: 1.5,
        top_km: 2.5,
        coverage: 0.8,
        kind: 0.6,
        wind: [0.0; 3],
        // Calm, with small cells: no region (a clear high, a hole between
        // cells) wider than the views.
        weather: PlanetWeather {
            mesoscale_km: 8.0,
            ..PlanetWeather::calm()
        },
        ..CloudLayer::cumulus()
    }
}

/// Over flat ground, the layer between 1.5 and 2.5 km (calm, its cells
/// small: its coverage wherever the view looks) changes the view
/// wherever a ray crosses the band and nowhere else: looking up from below
/// and down from above (onto the planet's analytic ground) the clouds show;
/// looking up from above them, or down from below, the sky is the clear
/// sky's.
#[test]
fn clouds_appear_in_their_band_and_nowhere_else() {
    let Some(mut view) = view(320, 180, false) else {
        return;
    };
    let validation = view.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let sun = Vec3::new(0.4, 0.7, 0.3).normalize();
    let aspect = 16.0 / 9.0;
    let up = Vec3::new(0.2, 1.0, 0.1);
    let down = Vec3::new(0.2, -1.0, 0.1);
    let level = Vec3::new(1.0, 0.0, 0.3);
    let cases = [
        ("below_up", Vec3::new(0.0, 300.0, 0.0), up, true),
        // Low in the band, where its form's clouds are thickest.
        ("inside_level", Vec3::new(0.0, 1800.0, 0.0), level, true),
        ("above_down", Vec3::new(0.0, 4000.0, 0.0), down, true),
        ("above_up", Vec3::new(0.0, 4000.0, 0.0), up, false),
        ("below_down", Vec3::new(0.0, 300.0, 0.0), down, false),
    ];
    for (name, eye, forward, crosses) in cases {
        let camera = camera(eye, forward, 60.0, aspect);
        view.set_sky(
            sun,
            AtmosphereComponent::from(helio_pass_sky::AtmosphereRow::earth()),
        );
        let clear = view.settle(&camera, 8);
        view.set_sky(
            sun,
            AtmosphereComponent::from(helio_pass_sky::AtmosphereRow::earth().with_clouds(band())),
        );
        let cloudy = view.settle(&camera, 16);
        view.capture(&format!("band_{name}_clear"), &clear);
        view.capture(&format!("band_{name}_cloudy"), &cloudy);
        let difference = mean_abs_difference(&clear, &cloudy);
        eprintln!("CLOUD_BAND {name}: mean |cloudy - clear| = {difference:.2}");
        if crosses {
            assert!(
                difference > 4.0,
                "{name}: no clouds where the view crosses the band ({difference:.2})"
            );
        } else {
            assert!(
                difference < 0.5,
                "{name}: clouds outside the band ({difference:.2})"
            );
        }
    }
    assert!(pollster::block_on(validation.pop()).is_none());
}

/// A still view of a cloudy sky holds still: after the clouds' history
/// settles, no pixel flickers from frame to frame.
#[test]
fn a_still_cloudy_sky_does_not_flicker() {
    let Some(mut view) = view(320, 180, false) else {
        return;
    };
    let sun = Vec3::new(0.5, 0.45, 0.2).normalize();
    view.set_sky(
        sun,
        AtmosphereComponent::from(
            helio_pass_sky::AtmosphereRow::earth().with_clouds(CloudLayer {
                coverage: 0.5,
                ..band()
            }),
        ),
    );
    let camera = camera(
        Vec3::new(0.0, 200.0, 0.0),
        Vec3::new(1.0, 0.45, 0.2),
        70.0,
        16.0 / 9.0,
    );
    let first = view.settle(&camera, 48);
    view.capture("still", &first);
    let frames: Vec<Vec<[u8; 4]>> = (0..12).map(|_| view.settle(&camera, 1)).collect();
    let pixels = first.len();
    let (mut sum_std, mut worst, mut jumpy) = (0.0, 0.0f64, 0usize);
    let mut map = vec![[0u8, 0, 0, 255]; pixels];
    for i in 0..pixels {
        let values: Vec<f64> = frames.iter().map(|f| luminance(f[i])).collect();
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let std = (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>()
            / values.len() as f64)
            .sqrt();
        let range = values.iter().cloned().fold(f64::MIN, f64::max)
            - values.iter().cloned().fold(f64::MAX, f64::min);
        sum_std += std;
        worst = worst.max(std);
        jumpy += usize::from(range > 6.0);
        map[i] = [
            (range * 8.0).min(255.0) as u8,
            first[i][1] / 4,
            first[i][2] / 4,
            255,
        ];
    }
    view.capture("still_flicker", &map);
    let mean_std = sum_std / pixels as f64;
    let jumpy_share = jumpy as f64 / pixels as f64;
    eprintln!(
        "CLOUD_FLICKER mean std {mean_std:.3}, worst std {worst:.2}, pixels changing > 6: {:.3}%",
        jumpy_share * 100.0
    );
    assert!(
        mean_std < 0.35,
        "the still sky flickers: mean std {mean_std:.3}"
    );
    assert!(
        jumpy_share < 0.002,
        "{:.3}% of the pixels flicker",
        jumpy_share * 100.0
    );
}

/// Pearson correlation.
fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    ab / (aa * bb).sqrt().max(1e-9)
}

/// The clouds' shadow lies on the ground along the sun: seen from below
/// the clouds looking down the sun's direction, the ground darkens where,
/// seen from that ground looking up towards the sun, clouds cover the sky.
/// Each ground pixel's point is followed along the sun to the clouds'
/// middle and found in the up view; followed straight up instead, it finds
/// clouds that do not shade it. The up view is lit by another sun, off
/// to the side (the clouds' shapes do not depend on it), so their cover is
/// not lost in the glare around the sun.
#[test]
fn a_clouds_shadow_darkens_the_ground_under_it_along_the_sun() {
    const SIZE: u32 = 128;
    let Some(mut view) = view(SIZE, SIZE, true) else {
        return;
    };
    let validation = view.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let planet = Arc::new(
        Planet::new(PlanetRecipe {
            shape: helio_pass_voxel_planet::grid::Shape::Plane,
            plane_size_m: 16384.0,
            terrain: helio_pass_voxel_planet::layers::TerrainLayers::flat().source(7),
            ..Default::default()
        })
        .unwrap(),
    );
    let sun = Vec3::new(0.35, 0.8, 0.2).normalize();
    let clouds = CloudLayer {
        bottom_km: 3.0,
        top_km: 4.5,
        coverage: 0.45,
        kind: 0.7,
        weather_km: 4.0,
        shape_km: 1.2,
        detail_km: 0.3,
        wind: [0.0; 3],
        ..CloudLayer::cumulus()
    };
    // The down view's eye: along the sun from the ground point, below the
    // clouds' base.
    let to_eye = 2700.0 / sun.y;
    let down_fov = 36.0f32;
    // Wide enough to hold the clouds along the sun and straight above.
    let up_fov = 100.0f32;
    let frame_up = sun.cross(Vec3::Y).normalize().cross(sun).normalize();
    let looking = |eye: Vec3, forward: Vec3, fov: f32| {
        Camera::perspective_look_at(
            eye,
            eye + forward,
            frame_up,
            fov.to_radians(),
            1.0,
            0.1,
            1.0e7,
        )
    };
    let ground_point = Vec3::new(0.0, 1.0, 0.0);
    let down = looking(ground_point + sun * to_eye, -sun, down_fov);
    let up = looking(ground_point, sun, up_fov);
    assert!(view.ground, "a view with ground");
    let settle_ground = |view: &mut View, camera: &Camera| {
        let frame = PlanetFrame::single(planet.clone(), sun);
        view.renderer
            .set_scene_payload(&VOXEL_FRAME, Arc::new(frame));
        let started = std::time::Instant::now();
        loop {
            view.render(camera);
            let pass = view.renderer.find_pass::<ChunkPass>().unwrap();
            if (pass.has_world() && pass.settled()) || started.elapsed().as_secs() > 300 {
                break;
            }
        }
    };
    let side_sun = Vec3::new(-0.5, 0.6, -0.3).normalize();
    let mut images = Vec::new();
    for air in [
        AtmosphereComponent::from(helio_pass_sky::AtmosphereRow::earth()),
        AtmosphereComponent::from(helio_pass_sky::AtmosphereRow::earth().with_clouds(clouds)),
    ] {
        view.set_sky(sun, air);
        settle_ground(&mut view, &down);
        let ground = view.settle(&down, 16);
        view.set_sky(side_sun, air);
        settle_ground(&mut view, &up);
        let sky = view.settle(&up, 16);
        images.push((ground, sky));
    }
    let [(clear_ground, clear_sky), (cloudy_ground, cloudy_sky)] =
        [images.remove(0), images.remove(0)];
    view.capture("shadow_ground_clear", &clear_ground);
    view.capture("shadow_ground_cloudy", &cloudy_ground);
    view.capture("shadow_sky_clear", &clear_sky);
    view.capture("shadow_sky_cloudy", &cloudy_sky);
    let (mut darkening, mut cover, mut cover_above) = (Vec::new(), Vec::new(), Vec::new());
    let n = SIZE as usize;
    let down_inverse = down.view_proj_in(glam::DVec3::ZERO).inverse();
    let up_projection = up.view_proj_in(glam::DVec3::ZERO);
    let clouds_middle = 3750.0;
    for y in 0..n {
        for x in 0..n {
            let ndc = glam::Vec2::new(
                (x as f32 + 0.5) / n as f32 * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / n as f32 * 2.0,
            );
            let unproject = |depth: f32| {
                let p = down_inverse * glam::Vec4::new(ndc.x, ndc.y, depth, 1.0);
                p.truncate() / p.w
            };
            let (near, far) = (unproject(1.0), unproject(0.5));
            let dir = (far - near).normalize();
            if dir.y >= -1e-3 {
                continue;
            }
            let ground = near + dir * ((ground_point.y - near.y) / dir.y);
            // The up view's pixel of a point, if it is in view.
            let pixel = |point: Vec3| {
                let clip = up_projection * point.extend(1.0);
                let (u, v) = (clip.x / clip.w * 0.5 + 0.5, 0.5 - clip.y / clip.w * 0.5);
                (clip.w > 0.0 && (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v))
                    .then(|| (v * n as f32) as usize * n + (u * n as f32) as usize)
            };
            let cover_at = |s: usize| (luminance(cloudy_sky[s]) - luminance(clear_sky[s])).abs();
            let along_sun = pixel(ground + sun * ((clouds_middle - ground.y) / sun.y));
            let above = pixel(ground + Vec3::Y * (clouds_middle - ground.y));
            let (Some(s), Some(a)) = (along_sun, above) else {
                continue;
            };
            let g = y * n + x;
            darkening.push(luminance(clear_ground[g]) - luminance(cloudy_ground[g]));
            cover.push(cover_at(s));
            cover_above.push(cover_at(a));
        }
    }
    assert!(
        darkening.len() > n * n / 4,
        "the views do not overlap: {}",
        darkening.len()
    );
    let mean_darkening = darkening.iter().sum::<f64>() / darkening.len() as f64;
    let r = correlation(&darkening, &cover);
    let r_above = correlation(&darkening, &cover_above);
    eprintln!(
        "CLOUD_SHADOW ground darkening {mean_darkening:.2}, correlation with the cover along the sun {r:.3}, straight above {r_above:.3} ({} pixels)",
        darkening.len()
    );
    assert!(
        mean_darkening > 1.5,
        "the clouds cast no shadow: {mean_darkening:.2}"
    );
    assert!(
        r > 0.5,
        "the shadow is not under the clouds along the sun: r = {r:.3}"
    );
    assert!(
        r > r_above + 0.2,
        "the shadow falls straight down, not along the sun: {r:.3} vs {r_above:.3}"
    );
    assert!(pollster::block_on(validation.pop()).is_none());
}

/// From orbit the clouds lie over the ground: 400 km above an Earth-sized
/// voxel planet, a cloud is a few kilometres in front of the terrain under
/// it, a sliver of its distance, and must not be taken for being behind it.
#[test]
fn clouds_seen_from_orbit_lie_over_the_ground() {
    const SIZE: u32 = 128;
    let Some(mut view) = view(SIZE, SIZE, true) else {
        return;
    };
    let validation = view.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let planet = Arc::new(Planet::new(PlanetRecipe::default()).unwrap());
    let radius = planet.grid().radius();
    let below = planet.surface_point(glam::DVec3::new(0.2, 1.0, 0.3), 0.0);
    let up = below.normalize();
    let sun = (up + up.any_orthonormal_vector() * 0.3)
        .normalize()
        .as_vec3();
    // Small, dense clouds: several in the 14 km the view holds.
    let clouds = CloudLayer {
        bottom_km: 2.0,
        top_km: 4.0,
        coverage: 0.7,
        weather_km: 4.0,
        shape_km: 1.2,
        detail_km: 0.3,
        wind: [0.0; 3],
        ..CloudLayer::cumulus()
    };
    let altitude = 400_000.0;
    let eye = below + up * altitude;
    let camera = Camera::perspective_look_to(
        eye,
        -up.as_vec3(),
        up.any_orthonormal_vector().as_vec3(),
        // About 14 km of ground.
        2.0 * (7000.0 / altitude as f32).atan(),
        1.0,
        1.0,
        1.0e9,
    );
    assert!(view.ground, "a view with ground");
    let frame = PlanetFrame::single(planet, sun);
    view.renderer
        .set_scene_payload(&VOXEL_FRAME, Arc::new(frame));
    let air = AtmosphereComponent::from(
        helio_pass_sky::AtmosphereRow::earth().around_planet([0.0; 3], radius),
    );
    let mut images = Vec::new();
    for air in [air, air.with_clouds(clouds).into()] {
        view.set_sky(sun, air);
        let started = std::time::Instant::now();
        loop {
            view.render(&camera);
            let pass = view.renderer.find_pass::<ChunkPass>().unwrap();
            if (pass.has_world() && pass.settled()) || started.elapsed().as_secs() > 300 {
                break;
            }
        }
        images.push(view.settle(&camera, 16));
    }
    view.capture("orbit_clear", &images[0]);
    view.capture("orbit_cloudy", &images[1]);
    let brightness = |pixels: &[[u8; 4]]| {
        pixels.iter().map(|p| luminance(*p)).sum::<f64>() / pixels.len() as f64
    };
    let (clear, cloudy) = (brightness(&images[0]), brightness(&images[1]));
    eprintln!("CLOUD_ORBIT from 400 km: mean luminance over the ground clear {clear:.2}, cloudy {cloudy:.2}");
    // Sunlit cloud tops are brighter than the ground they hide.
    assert!(
        cloudy > clear + 4.0,
        "no clouds over the ground from orbit ({clear:.2} -> {cloudy:.2})"
    );
    assert!(pollster::block_on(validation.pop()).is_none());
}

/// From far away the cover is the planet's weather, not a tile: an
/// Earth-sized planet seen whole, its clouds' cover (cloudy against clear)
/// averaged over blocks some 800 km across varies from block to block
/// (systems, bands, clear highs), where a calm sky's (the layer's coverage
/// everywhere, its cells and groups alone) is about even.
#[test]
fn from_far_away_the_cover_is_weather_not_a_tile() {
    const SIZE: u32 = 192;
    const BLOCK: usize = 12;
    let Some(mut view) = view(SIZE, SIZE, false) else {
        return;
    };
    let validation = view.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let radius = 6_371_000.0f64;
    let distance = 4.0 * radius;
    let towards = glam::DVec3::new(0.3, 0.6, 1.0).normalize();
    let eye = towards * distance;
    let fov = 2.0 * (radius / distance).asin() as f32 * 1.1;
    let camera =
        Camera::perspective_look_to(eye, -towards.as_vec3(), Vec3::Y, fov, 1.0, 1000.0, 1.0e9);
    let sun = (towards.as_vec3() + Vec3::new(0.3, 0.2, 0.0)).normalize();
    let air = AtmosphereComponent::from(
        helio_pass_sky::AtmosphereRow::earth().around_planet([0.0; 3], radius),
    );
    let clouds = CloudLayer {
        wind: [0.0; 3],
        ..CloudLayer::cumulus()
    };
    let mut images = Vec::new();
    for air in [
        air,
        air.with_clouds(clouds).into(),
        air.with_clouds(CloudLayer {
            weather: PlanetWeather::calm(),
            ..clouds
        })
        .into(),
    ] {
        view.set_sky(sun, air);
        images.push(view.settle(&camera, 24));
    }
    view.capture("far_clear", &images[0]);
    view.capture("far_weather", &images[1]);
    view.capture("far_calm", &images[2]);
    // Blocks well inside the disk (its middle half across).
    let n = SIZE as usize;
    let variation = |cloudy: &[[u8; 4]]| {
        let mut means = Vec::new();
        for by in (0..n).step_by(BLOCK) {
            for bx in (0..n).step_by(BLOCK) {
                let centre = (
                    (bx + BLOCK / 2) as f64 / n as f64 - 0.5,
                    (by + BLOCK / 2) as f64 / n as f64 - 0.5,
                );
                if (centre.0 * centre.0 + centre.1 * centre.1).sqrt() > 0.22 {
                    continue;
                }
                let mut sum = 0.0;
                for y in by..by + BLOCK {
                    for x in bx..bx + BLOCK {
                        sum +=
                            (luminance(cloudy[y * n + x]) - luminance(images[0][y * n + x])).abs();
                    }
                }
                means.push(sum / (BLOCK * BLOCK) as f64);
            }
        }
        let mean = means.iter().sum::<f64>() / means.len() as f64;
        let std = (means.iter().map(|m| (m - mean) * (m - mean)).sum::<f64>() / means.len() as f64)
            .sqrt();
        (mean, std / mean.max(1e-6), means.len())
    };
    let (weather_mean, weather, blocks) = variation(&images[1]);
    let (calm_mean, calm, _) = variation(&images[2]);
    eprintln!(
        "CLOUD_FAR {blocks} blocks: weather cover {weather_mean:.1}, variation {weather:.3}; calm cover {calm_mean:.1}, variation {calm:.3}"
    );
    assert!(
        weather_mean > 4.0 && calm_mean > 4.0,
        "no clouds from afar ({weather_mean:.1}, {calm_mean:.1})"
    );
    assert!(
        weather > 0.15,
        "the weather's cover is even from afar: {weather:.3}"
    );
    assert!(
        weather > 2.0 * calm,
        "the weather varies no more than a calm sky: {weather:.3} vs {calm:.3}"
    );
    assert!(pollster::block_on(validation.pop()).is_none());
}

/// What the clouds cost at 1080p looking at a cloudy horizon from the
/// ground and from orbit (400 km over an Earth-sized planet, down towards
/// its horizon): their trace (`CloudTrace`, the sun's visibility included)
/// and the atmosphere pass's extra (noise once, the planet's weather, the
/// shadow map each frame) and the composite, against the same view without
/// clouds. Printed; bounded loosely, as test
/// machines differ (the target is under 1 ms on the dev GPU).
#[test]
fn clouds_cost() {
    let Some(mut view) = view(1920, 1080, false) else {
        return;
    };
    if !view
        .device
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY)
    {
        eprintln!("CLOUD_COST skipped: no timestamp queries");
        return;
    }
    let sun = Vec3::new(0.5, 0.35, 0.2).normalize();
    let camera = camera(
        Vec3::new(0.0, 300.0, 0.0),
        Vec3::new(1.0, 0.25, 0.3),
        70.0,
        16.0 / 9.0,
    );
    let measure = |view: &mut View, camera: &Camera, air: AtmosphereComponent| {
        view.set_sky(sun, air);
        let mut samples: std::collections::HashMap<&'static str, Vec<f32>> = Default::default();
        let mut last = None;
        for frame in 0..240 {
            view.render(camera);
            let snapshot = view.renderer.timing_snapshot();
            if frame < 60 || snapshot.gpu_frame_index == last {
                continue;
            }
            last = snapshot.gpu_frame_index;
            for pass in &snapshot.passes {
                if let Some(ms) = pass.gpu_ms {
                    samples.entry(pass.name).or_default().push(ms);
                }
            }
        }
        samples
            .into_iter()
            .map(|(name, mut v)| {
                v.sort_by(f32::total_cmp);
                (name, v[v.len() / 2])
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let get = |m: &std::collections::HashMap<&str, f32>, k: &str| m.get(k).copied().unwrap_or(0.0);
    let radius = 6_371_000.0f64;
    let orbit = Camera::perspective_look_to(
        glam::DVec3::new(0.0, radius + 400_000.0, 0.0),
        Vec3::new(1.0, -0.55, 0.2),
        Vec3::Y,
        70f32.to_radians(),
        16.0 / 9.0,
        1.0,
        1.0e9,
    );
    let views = [
        (
            "ground",
            camera,
            AtmosphereComponent::from(helio_pass_sky::AtmosphereRow::earth()),
        ),
        (
            "orbit",
            orbit,
            AtmosphereComponent::from(
                helio_pass_sky::AtmosphereRow::earth().around_planet([0.0; 3], radius),
            ),
        ),
    ];
    for (name, camera, air) in views {
        let clear = measure(&mut view, &camera, air);
        let cloudy = measure(
            &mut view,
            &camera,
            air.with_clouds(CloudLayer::cumulus()).into(),
        );
        let trace = get(&cloudy, "CloudTrace");
        let atmosphere = get(&cloudy, "Atmosphere") - get(&clear, "Atmosphere");
        let composite = get(&cloudy, "AtmosphereComposite") - get(&clear, "AtmosphereComposite");
        let total = trace + atmosphere.max(0.0) + composite.max(0.0);
        eprintln!(
            "CLOUD_COST {name} 1920x1080: trace {trace:.3} ms (clear sky {:.3}), atmosphere +{atmosphere:.3} ms, composite +{composite:.3} ms, total {total:.3} ms",
            get(&clear, "CloudTrace")
        );
        assert!(trace > 0.0, "{name}: no timing for the cloud trace");
        assert!(total < 4.0, "{name}: clouds cost {total:.3} ms");
    }
}
