//! A northern lake under Helio's full sky: the atmosphere, its volumetric
//! clouds, the lake, a pine forest, fog and light shafts, filmed through the
//! lens. A capture tool: it renders each view until the temporal resolve has
//! converged, saves it, and moves on.
//!
//! Environment:
//!   MUSKOKA_ASSETS   the downloaded kit (models/, tex/)
//!   MUSKOKA_TOD      dawn | noon | golden | night
//!   MUSKOKA_VIEWS    comma list of view names (default: all)
//!   MUSKOKA_FRAMES   frames to render per view (default 100)
//!   MUSKOKA_OUT      output directory
//!   MUSKOKA_DENSITY  planting density, 1.0 is the full forest
//!   WATER_CAPTURE_SIZE  WIDTHxHEIGHT of the render (default 2560x1440)

mod common;
mod forest_scene;
mod lens_rows;
mod vg_support;

use common::{
    new_scene_db_with_gpu_mirror_and, point_light, scene_db_handle, spawn_light,
    spawn_water_volume, SurfaceCapture, WaterVolumeDescriptor,
};
use helio::{Camera, Renderer, RendererBuilder, RendererConfig};
use helio_pass_sky::{AtmosphereRow, CloudLayer, PlanetWeather};
use helio_pass_water_sim::WaterSimPass;
use pulsar_scenedb::{Entity, SceneDb};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    env_logger::init();
    // The surface needs COPY_SRC from the start to be read back.
    if std::env::var_os("HELIO_CAPTURE").is_none() {
        std::env::set_var("HELIO_CAPTURE", "unused.png");
    }
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App { state: None };
    event_loop.run_app(&mut app).expect("event loop");
}

struct App {
    state: Option<AppState>,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    renderer: Renderer,
    scene_db: SceneDb,
    lens: Entity,
    views: Vec<forest_scene::View>,
    view_index: usize,
    frames_per_view: u32,
    out_dir: PathBuf,
    tod: String,
    capture: Option<SurfaceCapture>,
    done: bool,
}

/// The sun's direction toward it from elevation and azimuth (degrees; azimuth 0
/// is the camera's default heading, -Z, and grows toward +X).
fn toward(elevation: f32, azimuth: f32) -> glam::Vec3 {
    let (e, a) = (elevation.to_radians(), azimuth.to_radians());
    glam::Vec3::new(a.sin() * e.cos(), e.sin(), -a.cos() * e.cos())
}

struct Tod {
    elevation: f32,
    azimuth: f32,
    sun_color: [f32; 3],
    sun_intensity: f32,
    ambient: [f32; 3],
    ambient_intensity: f32,
    exposure: f32,
    clouds: CloudLayer,
    water_color: [f32; 3],
    extinction: [f32; 3],
    /// Height fog: extinction (1/m), falloff (1/m), forward scattering.
    mist: (f32, f32, f32),
}

fn weather(seed: u32, cirrus: f32) -> PlanetWeather {
    PlanetWeather {
        seed,
        cirrus,
        evolution: 0.0,
        ..PlanetWeather::earth()
    }
}

fn tod(name: &str) -> Tod {
    let seed = std::env::var("MUSKOKA_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(11);
    let base = CloudLayer {
        bottom_km: 1.6,
        top_km: 4.4,
        coverage: 0.45,
        kind: 0.85,
        extinction: 45.0,
        weather_km: 20.0,
        shape_km: 5.0,
        detail_km: 0.7,
        wind: [0.3, 0.0, 0.15],
        albedo: [0.99; 3],
        erosion: 0.5,
        representation: helio_pass_sky::CloudRepresentation::Volumetric,
        weather: weather(seed, 0.55),
    };
    let tannin = ([0.006, 0.028, 0.030], [0.55, 0.16, 0.10]);
    match name {
        "dawn" => Tod {
            elevation: 4.5,
            azimuth: 32.0,
            sun_color: [1.0, 0.95, 0.88],
            sun_intensity: 5.0,
            ambient: [0.5, 0.55, 0.7],
            ambient_intensity: 0.12,
            exposure: 3.4,
            clouds: CloudLayer {
                coverage: 0.38,
                kind: 0.55,
                bottom_km: 1.2,
                top_km: 3.2,
                weather: weather(seed, 0.4),
                ..base
            },
            water_color: tannin.0,
            extinction: tannin.1,
            mist: (0.0032, 0.045, 0.6),
        },
        "golden" => Tod {
            elevation: 7.0,
            azimuth: 322.0,
            sun_color: [1.0, 0.95, 0.88],
            sun_intensity: 5.0,
            ambient: [0.5, 0.55, 0.7],
            ambient_intensity: 0.12,
            exposure: 3.2,
            clouds: CloudLayer {
                coverage: 0.5,
                kind: 0.8,
                weather: weather(seed, 0.7),
                ..base
            },
            water_color: tannin.0,
            extinction: tannin.1,
            mist: (0.0011, 0.02, 0.65),
        },
        "night" => Tod {
            elevation: 40.0,
            azimuth: 150.0,
            sun_color: [0.55, 0.65, 0.95],
            sun_intensity: 0.05,
            ambient: [0.2, 0.28, 0.5],
            ambient_intensity: 2.5,
            exposure: 5.0,
            clouds: CloudLayer {
                coverage: 0.08,
                weather: weather(seed, 0.1),
                ..base
            },
            water_color: [0.004, 0.012, 0.012],
            extinction: tannin.1,
            mist: (0.0006, 0.02, 0.5),
        },
        _ => Tod {
            elevation: 56.0,
            azimuth: 205.0,
            sun_color: [1.0, 0.97, 0.9],
            sun_intensity: 5.0,
            ambient: [0.5, 0.58, 0.75],
            ambient_intensity: 0.1,
            exposure: 2.3,
            clouds: base,
            water_color: tannin.0,
            extinction: tannin.1,
            mist: (0.0004, 0.012, 0.55),
        },
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let (width, height) = std::env::var("WATER_CAPTURE_SIZE")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((2560u32, 1440u32));
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio sky showcase")
                        .with_inner_size(winit::dpi::PhysicalSize::new(width, height)),
                )
                .expect("window"),
        );
        let size = window.inner_size();
        let usage = SurfaceCapture::from_env().usage();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Muskoka".into(),
                surface_usage: usage,
                ..Default::default()
            },
        ))
        .expect("graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;

        let name = std::env::var("MUSKOKA_TOD").unwrap_or_else(|_| "noon".into());
        let tod = tod(&name);

        let mut config = RendererConfig::new(size.width, size.height, format).with_feature(
            helio_light_api::ShadowSettings {
                quality: helio_light_api::ShadowQuality::High,
                ..Default::default()
            },
        );
        if std::env::var_os("MUSKOKA_NO_TSR").is_none() {
            config = config.with_tsr_quality(helio_pass_tsr::TsrQuality::Native);
        }
        let mut scene_db = new_scene_db_with_gpu_mirror_and(&device, &queue, |store| {
            helio_pass_water_sim::host_rows::WaterVolumeComponent::register_gpu_columns_growable(
                store, 8, &device,
            );
            lens_rows::LensComponent::register_gpu_columns_growable(store, 64, &device);
            lens_rows::LensElementComponent::register_gpu_columns_growable(store, 64, &device);
            helio_pass_volumetric_fog::host_rows::GlobalFogComponent::register_gpu_columns_growable(
                store, 16, &device,
            );
            helio_pass_volumetric_fog::host_rows::LocalFogVolumeComponent::register_gpu_columns_growable(store, 16, &device);
            helio_pass_volumetric_fog::host_rows::VolumetricFogSettingsComponent::register_gpu_columns_growable(store, 16, &device);
        });
        let textures = Arc::new(std::sync::RwLock::new(
            pulsar_scenedb::gpu::TextureStore::new(8192),
        ));
        let mirror = scene_db_handle(&scene_db)
            .with_texture_store(textures)
            .expect("texture store");
        scene_db.world.attach_gpu_mirror(mirror);
        // Virtual geometry carries the scanned trees: no triangle cap.
        let mut vg = (std::env::var("MUSKOKA_VG").as_deref() != Ok("0")
            && vg_support::Vg::supported(&device))
        .then(|| {
            vg_support::Vg::new(PathBuf::from(
                "D:/GitHub/Pulsar-Native/target/muskoka-assets/vgcache",
            ))
        });
        if let Some(vg) = &vg {
            config = vg.config(config);
        }
        let mut builder = RendererBuilder::new(config, scene_db_handle(&scene_db));
        if let Some(vg) = &vg {
            builder = builder.with_scene_derivation(vg.derivation(&device));
        }
        let mut renderer = builder
            .with_graph(helio_mesh_host::presets::with(
                helio_presets::Preset::Deferred,
                &common::features()
                    .with(helio_pass_volumetric_fog::FogFeature)
                    .with(helio_pass_lens::LensFeature)
                    .with(helio_pass_flare::LensFlareFeature),
            ))
            .build_with_graphics_context(&mut graphics)
            .expect("renderer");
        let surface = graphics.surface;

        // ── the sky ──
        let sky_id = scene_db.world.spawn();
        let mut row = AtmosphereRow::earth().with_clouds(tod.clouds);
        row.ground_albedo = [0.12, 0.13, 0.11];
        if name == "night" {
            row.night_sky_enabled = 1;
            row.star_count = 9000.0;
            row.star_brightness = 3.2;
            row.shooting_star_rate = 0.05;
            if helio_pass_sky::sky_texture::presets_available() {
                row.sky_texture_source = helio_pass_sky::sky_texture::source::GALAXY_NEBULA;
                helio_pass_sky::sky_texture::set_requested(row.sky_texture_source);
                row.sky_texture_intensity = 28.0;
                row.sky_texture_opacity = 1.0;
                row.sky_texture_yaw = 3.0;
                row.sky_texture_pitch = 0.3;
                row.sky_texture_roll = 1.0;
            }
        } else {
            row.night_sky_enabled = 0;
            row.shooting_stars_enabled = 0;
        }
        scene_db.world.insert(
            sky_id,
            helio_pass_sky::host_rows::AtmosphereComponent::from(row),
        );

        // ── light: the sun (or moon), with a shadow map and shafts ──
        let to_sun = toward(tod.elevation, tod.azimuth);
        let travel = -to_sun;
        spawn_light(
            &mut scene_db.world,
            helio_light_api::GpuLight {
                position_range: [0.0, 0.0, 0.0, f32::MAX],
                direction_outer: [travel.x, travel.y, travel.z, 0.0],
                color_intensity: [
                    tod.sun_color[0],
                    tod.sun_color[1],
                    tod.sun_color[2],
                    tod.sun_intensity,
                ],
                shadow_index: 0,
                light_type: helio_light_api::LightType::Directional as u32,
                god_rays_enabled: 1,
                god_rays_density: 1.0,
                god_rays_weight: 1.0,
                god_rays_exposure: 1.0,
                god_rays_decay: 1.0,
                ..Default::default()
            },
        );
        let views = {
            let all = forest_scene::views();
            match std::env::var("MUSKOKA_VIEWS") {
                Ok(list) => {
                    let wanted: Vec<&str> = list.split(',').map(str::trim).collect();
                    all.into_iter()
                        .filter(|v| wanted.contains(&v.name))
                        .collect()
                }
                Err(_) => all,
            }
        };
        if name == "night" {
            // Cool fill from above where the cameras stand (not directional, so
            // it does not brighten the sky).
            for v in &views {
                let p = v.position + glam::Vec3::new(0.0, 14.0, 0.0);
                spawn_light(
                    &mut scene_db.world,
                    point_light([p.x, p.y, p.z], [0.55, 0.68, 1.0], 260.0, 180.0),
                );
            }
        }

        // ── mist and light shafts ──
        let (extinction, falloff, anisotropy) = tod.mist;
        let fog = scene_db.world.spawn();
        scene_db.world.insert(
            fog,
            helio_pass_volumetric_fog::host_rows::GlobalFogComponent {
                enabled: (std::env::var_os("MUSKOKA_NOFOG").is_none()) as u32,
                mode: 1,
                extinction,
                height_falloff: falloff,
                height: 0.0,
                anisotropy,
                albedo: [0.82, 0.88, 1.0],
                ..Default::default()
            },
        );
        let fog_settings = scene_db.world.spawn();
        scene_db.world.insert(
            fog_settings,
            helio_pass_volumetric_fog::host_rows::VolumetricFogSettingsComponent {
                quality: 1,
                max_distance: 1800.0,
                light_max_distance: 1800.0,
                ..Default::default()
            },
        );

        // ── the lake ──
        let lake = WaterVolumeDescriptor {
            bounds_min: [-3000.0, -30.0, -3000.0],
            bounds_max: [3000.0, 40.0, 3000.0],
            surface_height: 0.0,
            wave_amplitude: 0.3,
            wave_frequency: 0.3,
            wave_speed: 1.0,
            wave_direction: [0.7, 0.7],
            wave_steepness: 0.3,
            water_color: tod.water_color,
            extinction: std::env::var("MUSKOKA_EXT")
                .ok()
                .and_then(|s| {
                    let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
                    (v.len() == 3).then(|| [v[0], v[1], v[2]])
                })
                .unwrap_or(tod.extinction),
            foam_threshold: 0.05,
            foam_amount: 0.4,
            reflection_strength: std::env::var("MUSKOKA_REFL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
            refraction_strength: 0.06,
            fresnel_power: 5.0,
            caustics_enabled: true,
            caustics_intensity: 0.7,
            caustics_scale: 4.5,
            caustics_speed: 0.4,
            fog_density: 0.0,
            god_rays_intensity: 0.1,
            ..Default::default()
        };
        let mut lake_row = lake.to_component();
        lake_row.sun_direction = [to_sun.x, to_sun.y, to_sun.z, tod.sun_intensity];
        let water_id = spawn_water_volume(&mut scene_db.world, lake);
        if let Some(mut water) = scene_db
            .world
            .get_mut::<helio_pass_water_sim::host_rows::WaterVolumeComponent>(water_id)
        {
            water.sun_direction = [to_sun.x, to_sun.y, to_sun.z, tod.sun_intensity];
        }
        if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
            // A lake: a light breeze, small chop, no breakers.
            sim.set_wind([0.7, 0.7], 0.5);
            sim.set_wave_scale(0.16);
            sim.set_wave_speed(1.0);
            sim.set_sim_dynamics(0.04, 0.990);
            sim.set_shore_swell(0.04);
        }

        // ── the land ──
        let root = std::env::var("MUSKOKA_ASSETS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("D:/GitHub/Pulsar-Native/target/muskoka-assets"));
        let density = std::env::var("MUSKOKA_DENSITY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);
        let started = std::time::Instant::now();
        forest_scene::build(
            &mut scene_db.world,
            &root,
            &device,
            &queue,
            density,
            &mut vg,
        )
        .expect("forest scene");
        eprintln!("scene built in {:.0?}", started.elapsed());
        // The water row once more, now the scene is in: the volume's first write
        // must not be the only one the GPU mirror sees.
        if let Some(mut water) = scene_db
            .world
            .get_mut::<helio_pass_water_sim::host_rows::WaterVolumeComponent>(water_id)
        {
            *water = lake_row;
        }
        if let Some(vg) = &vg {
            eprintln!(
                "virtual geometry: {} instances, {:.1}M source triangles",
                vg.instances,
                vg.triangles as f64 / 1e6
            );
        }
        if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
            sim.set_shore_map(Some(
                helio_pass_water_sim::WaterShoreMap::from_terrain(
                    [0.0, 0.0],
                    [1600.0, 1600.0],
                    0.0,
                    &forest_scene::Lake,
                )
                .expect("shoreline bake"),
            ));
            sim.set_ocean_rest_height(0.0).expect("rest height");
        }

        renderer.set_feature_live(helio::AmbientLight {
            color: tod.ambient,
            intensity: tod.ambient_intensity,
        });
        renderer.set_clear_color([0.0025, 0.0035, 0.01, 1.0]);
        let mut post = helio_pass_postprocess::PostProcessSettings::default();
        post.tonemap_operator = helio::TonemapOperator::AgxPunchy;
        post.tonemap_exposure = tod.exposure;
        post.color_contrast = [1.08; 3];
        post.color_saturation = [1.04; 3];
        post.bloom_enabled = true;
        post.bloom_intensity = 0.12;
        post.bloom_threshold = 1.1;
        post.bloom_radius = 1.2;
        post.vignette_enabled = false;
        post.grain_enabled = false;
        post.local_exposure_shadow_contrast = 0.7;
        renderer.set_scene_payload(&helio_pass_postprocess::POST_BASELINE, Arc::new(post));

        // ── the lens: 35 mm f/5.6, focused per view ──
        let lens_entity = {
            use helio_pass_lens::{LensElementRow, LensRegion::FullFrame as F};
            let mut lens = lens_rows::LensComponent::from(helio_pass_lens::LensRow::new(0));
            lens.focal_length_mm = 35.0;
            lens.f_number = 5.6;
            lens.t_stop = 5.6;
            lens.quality = 2;
            lens.aperture_blades = 7;
            lens.max_coc_px = 14.0 * (width as f32 / 2560.0);
            let elements = [
                LensElementRow::natural_vignette(0.12, F),
                LensElementRow::lateral_ca(0.0012, 0.0, F),
                LensElementRow::pro_mist(0.14, F),
                LensElementRow::halation(0.35, 0.012, F),
                LensElementRow::corner_softness(0.0025, 0.6, F),
            ];
            let slots: Vec<_> = (0..8).map(|_| scene_db.world.spawn()).collect();
            lens.element_first = slots[0].index();
            lens.element_count = elements.len() as u32;
            for (i, slot) in slots.iter().enumerate() {
                let element = elements
                    .get(i)
                    .map_or_else(bytemuck::Zeroable::zeroed, |e| {
                        lens_rows::LensElementComponent::from(*e)
                    });
                scene_db.world.insert(*slot, element);
            }
            let entity = scene_db.world.spawn();
            scene_db.world.insert(entity, lens);
            entity
        };

        let out_dir = std::env::var("MUSKOKA_OUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("D:/GitHub/Pulsar-Native/target/muskoka-out"));
        std::fs::create_dir_all(&out_dir).expect("output directory");
        let frames_per_view = std::env::var("MUSKOKA_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);

        let mut state = AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            usage,
            renderer,
            scene_db,
            lens: lens_entity,
            views,
            view_index: 0,
            frames_per_view,
            out_dir,
            tod: name,
            capture: None,
            done: false,
        };
        state.arm();
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        usage: state.usage,
                        format: state.surface_format,
                        width: s.width,
                        height: s.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Auto,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.renderer.set_render_size(s.width, s.height);
            }
            WindowEvent::RedrawRequested => {
                state.render();
                if state.done {
                    event_loop.exit();
                } else {
                    state.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}

impl AppState {
    /// Points the lens and the capture at the current view.
    fn arm(&mut self) {
        let Some(view) = self.views.get(self.view_index) else {
            self.done = true;
            return;
        };
        let path = self.out_dir.join(format!("{}-{}.png", self.tod, view.name));
        std::env::set_var("HELIO_CAPTURE", &path);
        std::env::set_var("HELIO_CAPTURE_FRAME", self.frames_per_view.to_string());
        self.capture = Some(SurfaceCapture::from_env());
        if let Some(mut lens) = self
            .scene_db
            .world
            .get_mut::<lens_rows::LensComponent>(self.lens)
        {
            lens.focus_distance_m = view.focus_m;
        }
        // A cut: nothing before it is history for the temporal resolve.
        if let Some(tsr) = self.renderer.find_pass_mut::<helio_pass_tsr::TsrPass>() {
            tsr.reset_history();
        }
        eprintln!(
            "view {} ({}/{})",
            view.name,
            self.view_index + 1,
            self.views.len()
        );
    }

    fn render(&mut self) {
        let Some(view) = self.views.get(self.view_index) else {
            self.done = true;
            return;
        };
        let (sy, cy) = view.yaw.sin_cos();
        let (sp, cp) = view.pitch.sin_cos();
        let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let size = self.window.inner_size();
        let camera = Camera::perspective_look_at(
            view.position,
            view.position + forward,
            glam::Vec3::Y,
            view.fov,
            size.width as f32 / size.height.max(1) as f32,
            0.1,
            30000.0,
        );
        common::flush_scene_db(&self.scene_db, &self.queue);
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => return,
        };
        if let Err(e) = self
            .renderer
            .render(&camera, &output.texture.create_view(&Default::default()))
        {
            log::error!("render error: {e:?}");
        }
        let saved = self
            .capture
            .as_mut()
            .is_some_and(|c| c.frame(&self.device, &self.queue, &output.texture));
        self.queue.present(output);
        if saved {
            self.view_index += 1;
            self.arm();
        }
    }
}
