//! Oil rig at night – water volume demonstration
//!
//! A platform rising from a dark ocean with underwater accent lights beneath the rig.
//! Includes nighttime sky and glowing light arrays on the rig legs to highlight caustics
//! and reflection behavior in the water scene actor.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod common;
mod coast_scene;
mod lens_rows;

use common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, point_light, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object, spawn_sky, spawn_water_volume,
    SurfaceCapture, WaterVolumeDescriptor,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_pass_water_sim::WaterSimPass;
use pulsar_scenedb::{Entity, SceneDb};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("Event loop error");
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
    renderer: Renderer,
    capture: SurfaceCapture,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,
    _light_ids: Vec<Entity>,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio Coastal Showcase")
                        .with_inner_size({
                            // WATER_CAPTURE_SIZE=2560x1440 renders captures at full res.
                            let (w, h) = std::env::var("WATER_CAPTURE_SIZE")
                                .ok()
                                .and_then(|s| {
                                    let (w, h) = s.split_once('x')?;
                                    Some((w.parse().ok()?, h.parse().ok()?))
                                })
                                .unwrap_or((1280u32, 720u32));
                            winit::dpi::PhysicalSize::new(w, h)
                        }),
                )
                .expect("window"),
        );

        let size = window.inner_size();
        let capture = SurfaceCapture::from_env();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Coastal Showcase".into(),
                surface_usage: capture.usage(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(size.width, size.height, format).with_feature(
            helio_light_api::ShadowSettings {
                quality: helio_light_api::ShadowQuality::High,
                ..Default::default()
            },
        );
        // WATER_TSR=1 switches to temporal super resolution. Off by default: the
        // water pass does not yet feed TSR (no surface motion vectors or
        // reactivity mask), so the sea smears and loses occlusion under it.
        let config = if std::env::var_os("WATER_TSR").is_some() {
            config.with_tsr_quality(helio_pass_tsr::TsrQuality::Native)
        } else {
            config
        };
        let mut scene_db = common::new_scene_db_with_gpu_mirror_and(&device, &queue, |store| {
            // The camera lens and its element block (the lens plugin's rows).
            lens_rows::LensComponent::register_gpu_columns_growable(store, 64, &device);
            lens_rows::LensElementComponent::register_gpu_columns_growable(store, 64, &device);
        });
        let textures = Arc::new(std::sync::RwLock::new(pulsar_scenedb::gpu::TextureStore::new(2048)));
        let mirror = scene_db_handle(&scene_db).with_texture_store(textures).expect("coast texture store");
        scene_db.world.attach_gpu_mirror(mirror);
        let mut renderer = RendererBuilder::new(config, scene_db_handle(&scene_db))
            // The water example's features plus the camera lens and its flares.
            .with_graph(helio_mesh_host::presets::with(
                helio_presets::Preset::Deferred,
                &common::features()
                    .with(helio_pass_lens::LensFeature)
                    .with(helio_pass_flare::LensFlareFeature),
            ))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        // Repeat addressing and 16x anisotropy: the sand and rock scans stay
        // sharp at grazing angles instead of smearing to their mean colour.
        // Opt-in (WATER_ANISO=1): overriding the material sampler currently makes the
        // water disappear, so the default sampler stays until that is understood.
        if std::env::var_os("WATER_ANISO").is_some() { renderer.set_feature(helio_mats::MaterialSampler(wgpu::SamplerDescriptor {
            label: Some("Coast repeat anisotropic sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        })); }

        let sky_id = spawn_sky(&mut scene_db.world, [1.0, 0.92, 0.78]);
        let time_of_day = std::env::var("WATER_TIME_OF_DAY").unwrap_or_else(|_| "day".into());
        // WATER_TIME_OF_DAY: day | dawn | sunset | overcast | night. Each carries
        // its own light, exposure, cloud cover, panorama and water tint.
        struct Tod {
            sun_direction: [f32; 3],
            sun_color: [f32; 3],
            sun_intensity: f32,
            ambient: [f32; 3],
            ambient_intensity: f32,
            exposure: f32,
            cloud: f32,
            hdri_opacity: f32,
            water_color: [f32; 3],
            extinction: [f32; 3],
        }
        let tod = match time_of_day.as_str() {
            "dawn" => Tod { sun_direction: [0.45, -0.42, 0.78], sun_color: [1.0, 0.7, 0.5], sun_intensity: 3.0, ambient: [0.62, 0.5, 0.52], ambient_intensity: 1.5, exposure: 3.0, cloud: 0.45, hdri_opacity: 0.85, water_color: [0.02, 0.07, 0.10], extinction: [0.30, 0.10, 0.05] },
            "sunset" => Tod { sun_direction: [-0.5, -0.36, 0.78], sun_color: [1.0, 0.62, 0.4], sun_intensity: 3.4, ambient: [0.62, 0.44, 0.4], ambient_intensity: 1.5, exposure: 3.2, cloud: 0.4, hdri_opacity: 0.85, water_color: [0.02, 0.06, 0.09], extinction: [0.30, 0.10, 0.05] },
            "overcast" => Tod { sun_direction: [0.4, -0.8, -0.4], sun_color: [0.85, 0.9, 1.0], sun_intensity: 0.8, ambient: [0.35, 0.38, 0.45], ambient_intensity: 0.9, exposure: 3.0, cloud: 0.95, hdri_opacity: 0.9, water_color: [0.03, 0.07, 0.08], extinction: [0.32, 0.12, 0.07] },
            "night" => Tod { sun_direction: [0.2, -0.7, -0.3], sun_color: [0.36, 0.47, 0.72], sun_intensity: 0.5, ambient: [0.12, 0.16, 0.3], ambient_intensity: 0.7, exposure: 3.4, cloud: 0.35, hdri_opacity: 0.95, water_color: [0.004, 0.02, 0.05], extinction: [0.38, 0.13, 0.03] },
            _ => Tod { sun_direction: [0.7, -0.58, -0.42], sun_color: [1.0, 0.93, 0.8], sun_intensity: 4.5, ambient: [0.14, 0.18, 0.26], ambient_intensity: 0.11, exposure: 2.3, cloud: 0.3, hdri_opacity: 0.0, water_color: [0.015, 0.11, 0.15], extinction: [0.26, 0.08, 0.04] },
        };
        let (sun_direction, sun_color, sun_intensity, ambient) = (tod.sun_direction, tod.sun_color, tod.sun_intensity, tod.ambient);
        let assets = std::env::var("WATER_BLOG_ASSETS").expect("set WATER_BLOG_ASSETS to the scan directory");
        // A real directional emitter drives atmospheric scattering and cloud
        // lighting. The HDR panorama below is a separate appearance layer.
        let sun = helio_light_api::GpuLight {
            position_range: [0.0, 0.0, 0.0, f32::MAX],
            direction_outer: [sun_direction[0], sun_direction[1], sun_direction[2], 0.0],
            color_intensity: [sun_color[0], sun_color[1], sun_color[2], sun_intensity],
            shadow_index: 0, // request the cascaded sun shadow map
            light_type: helio_light_api::LightType::Directional as u32,
            ..Default::default()
        };
        spawn_light(&mut scene_db.world, sun);
        if let Some(mut sky) = scene_db
            .world
            .get_mut::<helio_pass_sky::host_rows::AtmosphereComponent>(sky_id)
        {
            sky.clouds_enabled = 1;
            sky.cloud_bottom = 1.2;
            sky.cloud_top = 5.5;
            sky.cloud_coverage = tod.cloud;
            sky.cloud_type = 0.78;
            sky.cloud_wind = [7.0, 0.0, -3.0];
            sky.night_sky_enabled = u32::from(time_of_day == "night");
            sky.shooting_stars_enabled = u32::from(time_of_day == "night");
            // A pure-sky Poly Haven panorama sits under the atmosphere for every
            // time but the procedural day. WATER_HDRI_PATH overrides the file.
            let hdri = std::env::var("WATER_HDRI_PATH")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| Path::new(&assets).join("hdri").join(format!("{time_of_day}.hdr")));
            if tod.hdri_opacity > 0.0 && hdri.exists() {
                sky.sky_texture_source = helio_pass_sky::sky_texture::custom_source(&hdri);
                helio_pass_sky::sky_texture::set_requested(sky.sky_texture_source);
                sky.sky_texture_intensity = 1.0;
                sky.sky_texture_opacity = tod.hdri_opacity;
            }
        }

        // Ocean water volume — mid-ocean night, Beaufort 4 (~25 km/h). Static
        // appearance goes in the SceneDB descriptor; the heightfield sim's own
        // wind/wave dynamics are pass-owned runtime state, set below via
        // `WaterSimPass` (see `WaterVolumeDescriptor`'s own doc for why).
        let ocean = WaterVolumeDescriptor {
            bounds_min: [-120.0, -20.0, -120.0],
            bounds_max: [120.0, 40.0, 120.0],
            surface_height: 0.0,

            // Legacy Gerstner (not used by heightfield sim, kept for compat)
            wave_amplitude: 0.8,
            wave_frequency: 0.28,
            wave_speed: 1.0,
            wave_direction: [0.97, 0.14],
            wave_steepness: 0.35,

            // Deep pelagic water: red absorbed ~3m, green ~8m, blue ~30m
            water_color: tod.water_color,
            extinction: tod.extinction,

            foam_threshold: 0.05,
            foam_amount: 1.3,

            // Night mirror reflection, seawater Fresnel
            reflection_strength: 0.92,
            refraction_strength: 0.06,
            fresnel_power: 5.5,

            caustics_enabled: true,
            caustics_intensity: 1.1,
            caustics_scale: 4.5,
            caustics_speed: 0.5,

            fog_density: 0.016,
            god_rays_intensity: 0.15,

            ..Default::default()
        };
        let water_id = spawn_water_volume(&mut scene_db.world, ocean);
        if let Some(mut water) = scene_db.world.get_mut::<helio_pass_water_sim::host_rows::WaterVolumeComponent>(water_id) {
            let direction = -glam::Vec3::from_array(sun_direction).normalize();
            water.sun_direction = [direction.x, direction.y, direction.z, sun_intensity];
        }

        // NNE wind, Beaufort 4.
        // wave_scale=0.45 => primary swell wavelength ~28m in the 240m domain.
        // wave_speed=1.0 => phase velocity ~15 m/s for the primary swell.
        // wind_strength=1.5 drives ~0.4m significant wave height.
        // wave_spring=0.04 => sqrt(0.04) * 112.5 m/s = ~22 m/s -- realistic ocean swell.
        // wave_damping=0.990 => moderate decay: waves persist ~2 s before damping out.
        if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
            sim.set_wind([0.97, 0.14], 1.5);
            sim.set_wave_scale(0.45);
            sim.set_wave_speed(1.0);
            sim.set_sim_dynamics(0.04, 0.990);
        }

        coast_scene::build(&mut scene_db.world, Path::new(&assets), &device, &queue).expect("coastal scan scene");
        if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
            sim.set_shore_map(Some(helio_pass_water_sim::WaterShoreMap::from_terrain(
                [0.0, 0.0], [512.0, 512.0], 0.0, &coast_scene::Beach,
            ).expect("shoreline bake")));
            sim.set_ocean_rest_height(0.0).expect("rest height");
        }
        let _light_ids = Vec::new();
        // Optional faint moon as directional component-like sky bloom (general ambient control)
        renderer.set_feature_live(helio::AmbientLight {
            color: ambient,
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
        post.vignette_intensity = 0.3;
        post.vignette_smoothness = 0.8;
        // Grain doubles as dither: it breaks up banding in the cloud gradients.
        post.grain_enabled = false;
        post.grain_intensity = 0.018;
        post.grain_response = 0.3;
        post.grain_size = 0.7;
        post.local_exposure_shadow_contrast = 0.7;
        renderer.set_scene_payload(&helio_pass_postprocess::POST_BASELINE, Arc::new(post));

        let view = std::env::var("WATER_CAPTURE_VIEW").unwrap_or_default();
        let (mut cam_pos, cam_yaw, cam_pitch) = match std::env::var("WATER_CAPTURE_VIEW").as_deref() {
            Ok("underwater") => (glam::Vec3::new(0.0, -2.0, 30.0), 0.0, 0.12),
            Ok("waterline") => (glam::Vec3::new(9.0, 0.9, 5.0), -0.35, 0.0),
            // Driftwood and shells in the foreground, stacks and cliffs behind.
            Ok("driftwood") => (glam::Vec3::new(-12.0, 0.7, 20.0), 0.55, -0.02),
            // Along the pier toward the open sea.
            Ok("pier") => (glam::Vec3::new(14.0, 3.0, 6.0), 0.6, -0.12),
            // High on the dunes, looking down the beach to the cove.
            Ok("dunes") => (glam::Vec3::new(-30.0, 6.5, 52.0), 0.35, -0.2),
            // Wide aerial over the whole cove.
            Ok("aerial") => (glam::Vec3::new(10.0, 34.0, 60.0), 0.0, -0.42),
            // Looking back at the cliffs from the surf.
            Ok("cliffs") => (glam::Vec3::new(0.0, 2.0, -2.0), 3.1, 0.12),
            Ok("side") => (glam::Vec3::new(-24.0, 7.0, 30.0), 0.52, -0.12),
            _ => (glam::Vec3::new(-3.0, 1.8, 17.0), 0.1, -0.06),
        };
        // Cameras stay above the sand: a view authored from the water side of
        // the slope can otherwise end up under the beach.
        if view != "underwater" {
            cam_pos.y = cam_pos.y.max(coast_scene::height(cam_pos.x, cam_pos.z) + 1.4);
        }

        // Camera optics for the stills: a 35 mm lens at f/5.6 focused per view, the
        // natural cos^4 falloff, a touch of lateral colour, a light Pro-Mist and
        // film halation. WATER_LENS=off renders the bare frame.
        if std::env::var("WATER_LENS").map_or(true, |v| v != "off") {
            use helio_pass_lens::{LensElementRow, LensRegion::FullFrame as F};
            let focus = match view.as_str() {
                "waterline" => 9.0,
                "driftwood" => 5.0,
                "pier" => 10.0,
                "dunes" => 22.0,
                "aerial" => 90.0,
                "cliffs" => 30.0,
                _ => 8.0,
            };
            let mut lens = lens_rows::LensComponent::from(helio_pass_lens::LensRow::new(0));
            lens.focal_length_mm = 35.0;
            lens.f_number = 5.6;
            lens.t_stop = 5.6;
            lens.focus_distance_m = focus;
            lens.quality = 2;
            lens.aperture_blades = 7;
            lens.max_coc_px = 14.0;
            let elements = [
                LensElementRow::natural_vignette(0.12, F),
                LensElementRow::lateral_ca(0.0012, 0.0, F),
                LensElementRow::pro_mist(0.14, F),
                LensElementRow::halation(0.35, 0.012, F),
                LensElementRow::corner_softness(0.0025, 0.6, F),
            ];
            // Elements are addressed as a contiguous block of rows.
            let slots: Vec<_> = (0..8).map(|_| scene_db.world.spawn()).collect();
            lens.element_first = slots[0].index();
            lens.element_count = elements.len() as u32;
            for (i, slot) in slots.iter().enumerate() {
                let element = elements
                    .get(i)
                    .map_or_else(bytemuck::Zeroable::zeroed, |e| lens_rows::LensElementComponent::from(*e));
                scene_db.world.insert(*slot, element);
            }
            let lens_entity = scene_db.world.spawn();
            scene_db.world.insert(lens_entity, lens);
        }

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            capture,
            last_frame: std::time::Instant::now(),
            cam_pos,
            cam_yaw,
            cam_pitch,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            _light_ids,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        ..
                    },
                ..
            } => {
                if state.cursor_grabbed {
                    state.cursor_grabbed = false;
                    let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                    state.window.set_cursor_visible(true);
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ks,
                        physical_key: PhysicalKey::Code(key),
                        ..
                    },
                ..
            } => match ks {
                ElementState::Pressed => {
                    state.keys.insert(key);
                }
                ElementState::Released => {
                    state.keys.remove(&key);
                }
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !state.cursor_grabbed {
                    let ok = state
                        .window
                        .set_cursor_grab(CursorGrabMode::Confined)
                        .or_else(|_| state.window.set_cursor_grab(CursorGrabMode::Locked))
                        .is_ok();
                    if ok {
                        state.window.set_cursor_visible(false);
                        state.cursor_grabbed = true;
                    }
                }
            }
            WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        usage: state.capture.usage(),
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
                let now = std::time::Instant::now();
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;
                state.render(dt);
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        let Some(state) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if state.cursor_grabbed {
                state.mouse_delta.0 += dx as f32;
                state.mouse_delta.1 += dy as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}

impl AppState {
    fn render(&mut self, dt: f32) {
        const SPEED: f32 = 6.0;
        const SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.4, 1.4);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);

        if self.keys.contains(&KeyCode::KeyW) {
            self.cam_pos += forward * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            self.cam_pos -= forward * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            self.cam_pos -= right * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            self.cam_pos += right * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::Space) {
            self.cam_pos += glam::Vec3::Y * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos -= glam::Vec3::Y * SPEED * dt;
        }

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            800.0,
        );

        common::flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self.capture.render_window_frame(
            &mut self.renderer,
            &camera,
            &self.surface,
        ) {
            log::error!("Render error: {:?}", e);
        }
    }
}


