//! Foliage demo — GPU-driven grass over an open field.
//!
//! Exercises the whole foliage authoring path: a registered foliage type, a layer, the
//! global wind clock, and a moving interactor that pushes grass aside. Placement, culling
//! and LOD selection all happen on the GPU; the per-frame CPU cost here is a camera
//! update, one wind tick and one interactor move, regardless of how many blades are drawn.
//!
//! Rendered through the OpenXR path when a headset is connected (see `examples/vr`), and
//! a plain desktop mirror with WASD + mouse look when OpenXR initialisation fails. The
//! deferred preset (`presets::deferred`) is used unchanged in both modes. In XR mode,
//! `render_window_frame()` drives both eyes and mirrors them into the app-owned window.
//!
//! Controls:
//!   WASD        — move forward/left/back/right (desktop mirror mode)
//!   Space/Shift — move up/down (desktop mirror mode)
//!   Q/E         — decrease/increase wind speed
//!   R           — toggle the roaming interactor
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit
//!   Left stick  — move (XR)
//!   Right stick — turn (XR)

mod common;

use common::{
    directional_light, make_material, new_scene_db_with_gpu_mirror, plane_mesh, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object, spawn_sky, sphere_mesh,
};
use helio::{
    Camera, HelioGraphicsConfig, HelioGraphicsContext, HelioXrInput, RenderMode, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_pass_foliage_place::host_rows::FoliageInteractorComponent;
use helio_pass_foliage_place::host_rows::FoliageLayerComponent;
use helio_pass_foliage_place::host_rows::FoliageTypeComponent;
use helio_pass_foliage_place::host_rows::FoliageWindComponent;
use helio_pass_foliage_place::{
    pack_kind_and_flags, FoliageKind, FoliageMaterial, GpuFoliageLayer, GpuFoliageType,
    FOLIAGE_FLAG_RECEIVES_INTERACTION, FOLIAGE_FLAG_TWO_SIDED,
};
use pulsar_scenedb::{Entity, SceneDb};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;
use std::sync::Arc;

/// Half-extent of the ground plane in metres.
const FIELD_HALF_EXTENT: f32 = 120.0;

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
    graphics: HelioGraphicsContext<'static>,
    device: Arc<wgpu::Device>,
    renderer: Renderer,
    last_frame: std::time::Instant,
    start_time: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    prev_keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    wind_speed: f32,
    interactor_enabled: bool,
    scene_db: SceneDb,
    interactor_entity: Entity,
    wind_entity: Entity,
    interactor_prev_pos: glam::Vec3,
    marker_object: Entity,

    _sun_light_id: Entity,

    /// True when a live OpenXR session is installed in the renderer.
    xr_active: bool,
    /// Controller actions, and a handle to the session to sync them against.
    #[cfg(not(target_arch = "wasm32"))]
    xr_input: Option<HelioXrInput>,
    /// Player locomotion, applied as the XR stage transform. Position is the stage
    /// origin in world space; yaw is snap/smooth turn about it.
    player_position: glam::Vec3,
    player_yaw: f32,
}

impl AppState {
    fn configure_surface(&mut self, width: u32, height: u32) {
        self.graphics.resize(&mut self.renderer, width, height);
    }

    /// Read the controllers and walk the player, then publish the result as the XR
    /// stage transform. Movement is head-relative on the yaw axis only, matching
    /// `examples/vr`.
    #[cfg(not(target_arch = "wasm32"))]
    fn update_locomotion(&mut self, dt: f32) {
        const MOVE_SPEED: f32 = 2.5;
        const TURN_SPEED: f32 = 1.8;
        const DEADZONE: f32 = 0.15;

        let Some(input) = &self.xr_input else {
            return;
        };
        let Ok(controls) = input.controller_state() else {
            return;
        };

        let deadzone = |v: glam::Vec2| -> glam::Vec2 {
            if v.length() < DEADZONE {
                glam::Vec2::ZERO
            } else {
                v
            }
        };
        let move_stick = deadzone(controls.left_stick);
        let turn_stick = deadzone(controls.right_stick);

        self.player_yaw -= turn_stick.x * TURN_SPEED * dt;

        let (sin, cos) = self.player_yaw.sin_cos();
        let forward = glam::Vec3::new(-sin, 0.0, -cos);
        let right = glam::Vec3::new(cos, 0.0, -sin);
        self.player_position += (forward * move_stick.y + right * move_stick.x) * MOVE_SPEED * dt;

        input.set_locomotion_transform(
            &mut self.renderer,
            glam::Mat4::from_translation(self.player_position)
                * glam::Mat4::from_rotation_y(self.player_yaw),
        );
    }
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
                        .with_title("Helio – Foliage")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("window"),
        );

        // `HELIO_FOLIAGE=0` builds the graph with no foliage passes at all, so the ground
        // and the rest of the scene can be judged without them. Worth having as a runtime
        // switch rather than a rebuild: "is this the foliage or the base renderer?" is the
        // first question to answer for any artefact in this demo, and answering it should
        // not cost a recompile.
        let foliage_enabled = std::env::var("HELIO_FOLIAGE")
            .map(|value| value != "0")
            .unwrap_or(true);
        eprintln!(
            "[foliage_demo] foliage passes: {}",
            if foliage_enabled { "ON" } else { "OFF" }
        );

        // Density is an allocation ceiling, so it is stated up front and the arena is
        // sized to hold it — 256 blades/m² over a 128 m ring is ~256 MiB of blade arena.
        // Overridable so the cost/quality trade is visible rather than baked in.
        let blades_per_m2 = std::env::var("HELIO_FOLIAGE_DENSITY")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(256.0);

        let size = window.inner_size();
        let mut graphics = pollster::block_on(HelioGraphicsContext::new(
            window.clone(),
            size.width,
            size.height,
            HelioGraphicsConfig {
                vr_support: true,
                application_name: "Helio Foliage Demo".into(),
                ..Default::default()
            },
        ))
        .expect("graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let mut config = RendererConfig::new(size.width, size.height, format)
            .with_render_mode(RenderMode::Deferred);
        config
            .features
            .edit::<helio_pass_foliage_place::FoliageSettings>()
            .enabled = foliage_enabled;
        config
            .features
            .edit::<helio_pass_foliage_place::FoliageSettings>()
            .tufts_per_m2 = Some(blades_per_m2 / 8.0);

        device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| {
            panic!("[GPU UNCAPTURED ERROR] {:?}", e);
        }));

        // Foliage authoring lives in SceneDB rows, same as every other kind of
        // scene content -- the pass owns these schemas and its GPU mirror
        // publishes insert/mutate/remove through the World.
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let builder = RendererBuilder::new(config.clone(), scene_db_handle(&scene_db))
            .with_graph(common::preset());
        let (mut renderer, xr_input) = graphics.build_renderer(builder, config).expect("renderer");
        let xr_active = graphics.is_xr();

        // Indoors, but the sky still drives ambient. `AtmospherePass` is in every graph
        // and reads this row from SceneDB each frame, so it can be spawned at any time.
        spawn_sky(&mut scene_db.world, [0.05, 0.07, 0.11]);

        // ── Ground ───────────────────────────────────────────────────────────
        // Flat for now: `FoliageTerrainPass` (the top-down height/slope capture the
        // placement shader samples) is a later phase, and until it exists placement falls
        // back to a plane at y=0. This mesh is what that fallback is pretending to be, so
        // the two agree and the grass sits on the ground rather than floating.
        let ground_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.16, 0.22, 0.10, 1.0], 0.95, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let ground_mesh = spawn_mesh(
            &mut scene_db.world,
            plane_mesh([0.0, 0.0, 0.0], FIELD_HALF_EXTENT),
        );
        // A 240 m plane is the case a single bounding sphere describes worst: the sphere's
        // radius is set by the diagonal, so it is enormous next to the geometry actually
        // inside it. It culls essentially nothing useful, and getting the radius even
        // slightly wrong deletes the entire ground the moment a corner leaves the frustum.
        // One object always being submitted costs a single draw; the alternative is a
        // whole-screen artefact -- bounds-based culling (a real radius covering the whole
        // plane) stands in for the removed `INSTANCE_FLAG_ALWAYS_VISIBLE` flag, same as
        // every other migrated demo's always-visible geometry.
        let _ = spawn_object(
            &mut scene_db.world,
            ground_mesh,
            ground_mat,
            glam::Mat4::IDENTITY,
            FIELD_HALF_EXTENT * std::f32::consts::SQRT_2,
        );

        // A visible marker for the roaming interactor, so the grass displacement has
        // something obviously attached to it.
        let marker_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.8, 0.2, 0.15, 1.0], 0.4, 0.0, [0.5, 0.05, 0.0], 2.0),
        );
        let marker_mesh = spawn_mesh(&mut scene_db.world, sphere_mesh([0.0, 0.0, 0.0], 0.6));
        let marker_object = spawn_object(
            &mut scene_db.world,
            marker_mesh,
            marker_mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.6, 0.0)),
            0.6,
        )
        .expect("marker object");

        // ── Foliage ──────────────────────────────────────────────────────────
        let grass_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.28, 0.46, 0.14, 1.0], 0.85, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        let mut grass = GpuFoliageType {
            // Tufts of eight blades or so: the density is in tufts.
            density: blades_per_m2 / 8.0,
            height_range: [0.18, 0.5],
            width_range: [0.012, 0.03],
            // Everything up to 35° of slope (a band on its cosine). Flat ground here, but
            // this is the knob that keeps grass off cliff faces once real terrain is
            // under it.
            slope_range: [35f32.to_radians().cos(), 1.0],
            lod_distances: [8.0, 0.0, 0.0, 120.0],
            // Blades have no trunk, so the sway band is off; flutter carries the
            // body of the motion and jitter the tips.
            wind_response: [0.0, 0.35, 1.0],
            interaction_stiffness: 6.0,
            material_id: grass_mat.index(),
            altitude_range: [f32::MIN, f32::MAX],
            density_layer: 0,
            kind_and_flags: pack_kind_and_flags(
                FoliageKind::Blade,
                FOLIAGE_FLAG_TWO_SIDED | FOLIAGE_FLAG_RECEIVES_INTERACTION,
            ),
            mesh_or_impostor_id: 0,
            base_color: 0,
            roughness_metallic: 0,
            ground_mask: 0,
            lean_range: [0.15, 0.8],
            curve: 0.5,
            fall_chance: 0.05,
            ..GpuFoliageType::default()
        };
        grass.set_blades_per_tuft([5, 12]);
        // The same surface as `grass_mat`, carried by the type row itself.
        grass.set_material(Some(FoliageMaterial {
            base_color: [0.28, 0.46, 0.14],
            roughness: 0.85,
            metallic: 0.0,
        }));
        let grass: FoliageTypeComponent = grass.into();
        let layer: FoliageLayerComponent = GpuFoliageLayer {
            bounds_min: [-FIELD_HALF_EXTENT, -1.0, -FIELD_HALF_EXTENT, 0.0],
            bounds_max: [FIELD_HALF_EXTENT, 4.0, FIELD_HALF_EXTENT, 1.0],
        }
        .into();

        let grass_entity = scene_db.world.spawn();
        scene_db.world.insert(grass_entity, grass);
        let layer_entity = scene_db.world.spawn();
        scene_db.world.insert(layer_entity, layer);

        let wind_speed = 2.0;
        let wind_entity = scene_db.world.spawn();
        scene_db.world.insert(
            wind_entity,
            FoliageWindComponent {
                direction_speed: [1.0, 0.0, 0.35, wind_speed],
                gust: [0.6, 0.25, 0.0, 0.05],
                time_prev_time: [0.0, 0.0],
                _pad: [0.0; 2],
            },
        );
        let interactor_entity = scene_db.world.spawn();
        scene_db.world.insert(
            interactor_entity,
            FoliageInteractorComponent::from(helio_pass_foliage_place::FoliageInteractorRow::new(
                glam::DVec3::ZERO,
                1.2,
                1.0,
                glam::Vec3::ZERO,
                1.0,
            )),
        );

        // ── Lighting ─────────────────────────────────────────────────────────
        let sun_light_id = spawn_light(
            &mut scene_db.world,
            directional_light([-0.35, -0.8, -0.5], [1.0, 0.96, 0.88], 3.0),
        );

        let state = AppState {
            window,
            graphics,
            device,
            renderer,
            last_frame: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 1.7, 12.0),
            cam_yaw: 0.0,
            cam_pitch: -0.1,
            keys: HashSet::new(),
            prev_keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            wind_speed,
            interactor_enabled: true,
            scene_db,
            interactor_entity,
            wind_entity,
            interactor_prev_pos: glam::Vec3::ZERO,
            marker_object,
            _sun_light_id: sun_light_id,
            xr_active,
            xr_input,
            player_position: glam::Vec3::ZERO,
            player_yaw: 0.0,
        };
        self.state = Some(state);
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
                state.configure_surface(s.width, s.height);
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;

                #[cfg(not(target_arch = "wasm32"))]
                if state.xr_active {
                    state.update_locomotion(dt);
                    state.update_foliage_frame(dt);
                    {
                        let camera = Camera::perspective_look_at(
                            glam::Vec3::new(0.0, 1.6, 0.0),
                            glam::Vec3::new(0.0, 1.6, -1.0),
                            glam::Vec3::Y,
                            std::f32::consts::FRAC_PI_4,
                            1.0,
                            0.1,
                            1000.0,
                        );
                        if let Err(e) = state.graphics.render_frame(&mut state.renderer, &camera) {
                            log::error!("[XR] render error: {e:?}");
                        }
                    }
                    state.mouse_delta = (0.0, 0.0);
                    state.window.request_redraw();
                    return;
                }

                state.render_desktop(dt);
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
    /// Advances wind + the roaming interactor and flushes the scene. Shared between
    /// the desktop and XR redraw paths — nothing here depends on how the camera for
    /// this frame is being driven.
    fn update_foliage_frame(&mut self, dt: f32) {
        const WIND_STEP: f32 = 4.0;

        // Guard against a hitch (or a breakpoint) producing a huge dt, which would jump
        // the wind clock far enough that the motion-vector pair describes a teleport and
        // TAA/TSR smears the whole screen for a frame.
        let dt = dt.clamp(0.0, 0.1);

        if self.keys.contains(&KeyCode::KeyQ) {
            self.wind_speed = (self.wind_speed - WIND_STEP * dt).max(0.0);
        }
        if self.keys.contains(&KeyCode::KeyE) {
            self.wind_speed = (self.wind_speed + WIND_STEP * dt).min(30.0);
        }
        if self.keys.contains(&KeyCode::KeyR) && !self.prev_keys.contains(&KeyCode::KeyR) {
            self.interactor_enabled = !self.interactor_enabled;
        }
        self.prev_keys = self.keys.clone();

        let time = self.start_time.elapsed().as_secs_f32();

        // ── Drive the foliage frame state ────────────────────────────────────
        // SceneDB is the only authored state. Mutating these pass-owned rows
        // updates the GPU mirror without a renderer-side resubmission call.
        if let Some(mut wind) = self
            .scene_db
            .world
            .get_mut::<FoliageWindComponent>(self.wind_entity)
        {
            wind.direction_speed[3] = self.wind_speed;
            wind.time_prev_time[1] = wind.time_prev_time[0];
            wind.time_prev_time[0] += dt;
        }

        let marker_pos = if self.interactor_enabled {
            let radius = 9.0;
            glam::Vec3::new(
                (time * 0.45).cos() * radius,
                0.6,
                (time * 0.45).sin() * radius,
            )
        } else {
            glam::Vec3::new(0.0, -50.0, 0.0)
        };

        // Velocity is passed rather than differenced GPU-side: it draws the pressed track
        // behind the marker (its wake).
        let velocity = if dt > 0.0 {
            (marker_pos - self.interactor_prev_pos) / dt
        } else {
            glam::Vec3::ZERO
        };
        self.interactor_prev_pos = marker_pos;

        if let Some(mut interactor) = self
            .scene_db
            .world
            .get_mut::<FoliageInteractorComponent>(self.interactor_entity)
        {
            interactor.set_motion(marker_pos.as_dvec3(), velocity);
        }
    }

    /// Desktop mirror path: WASD + mouse free camera, then a normal render.
    fn render_desktop(&mut self, dt: f32) {
        const SPEED: f32 = 8.0;
        const SENS: f32 = 0.002;

        let dt = dt.clamp(0.0, 0.1);

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

        self.update_foliage_frame(dt);

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            1000.0,
        );

        // Upload this frame's SceneDB writes to the GPU mirror; without it they
        // are never visible to the renderer (Helio#266).
        common::flush_scene_db(&self.scene_db, self.renderer.queue());
        if let Err(e) = self.graphics.render_frame(&mut self.renderer, &camera) {
            log::error!("Render: {:?}", e);
        }
    }
}
