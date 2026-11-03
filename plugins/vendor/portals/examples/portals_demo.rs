//! Portals demo — a hallway with a portal at each end, each leading to a
//! colour-coded room beyond the corridor.
//!
//! Reuses the corridor geometry from `indoor_corridor.rs` (36 m long, 4 m
//! wide, 3 m tall). A portal sits just inside each end wall; through either
//! one you see a real, clipped, fully-lit duplicate of the room past the
//! *other* end — drawn in the same G-buffer pass, same depth buffer, no
//! separate camera (see `helio-pass-portal-instances`). Walk into either
//! portal and you're teleported to the other end.
//!
//! The far (blue) room beyond -Z is what shows through the near (+Z) portal;
//! the near (amber) room beyond +Z shows through the far (-Z) portal. Each
//! room only exists past its corridor end, so the mapped duplicate never
//! overlaps the real corridor — the fragment z-clip discards anything on the
//! near side of the linked surface, which is what would otherwise double the
//! tunnel.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod common;

use helio_pass_portal_cull::portal_pose_facing;
use helio::{
    required_experimental_features, required_wgpu_features,
    required_wgpu_limits, Camera, Renderer, RendererBuilder, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb};
use common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, point_light, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object,
};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;
use std::sync::Arc;

/// Corridor cross-section half-extent (matches the wall/floor/ceiling
/// geometry below) — also the portal clip opening's half-extent.
const HALF_WIDTH: f32 = 2.0;
const HALF_HEIGHT: f32 = 1.5;
/// Distance from centre to each end wall.
const HALF_LENGTH: f32 = 18.0;
/// Portal surfaces sit just inside the end walls, not flush with them, so
/// the clip plane doesn't fight the wall's own geometry.
const PORTAL_Z: f32 = HALF_LENGTH - 1.0;

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
    capture: common::SurfaceCapture,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    scene_db: SceneDb,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    /// Duplicates content near the far end (-Z) so it's visible through the
    /// near portal (+Z).
    portal_near: helio_pass_portal_cull::PortalPair,
    /// Duplicates content near the near end (+Z) so it's visible through the
    /// far portal (-Z).
    portal_far: helio_pass_portal_cull::PortalPair,

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
                        .with_title("Helio – Portals")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                surface_usage: common::SurfaceCapture::from_env().usage(),
                application_name: "Portals Demo".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;

        let mut config = RendererConfig::new(size.width, size.height, format);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let mut renderer = RendererBuilder::new(config, scene_db_handle(&scene_db))
            .with_graph(common::preset())
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.72, 0.72, 0.75, 1.0], 0.8, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        // Corridor: 4 m wide (X), 3 m tall (Y), 36 m long (Z: -18..+18) — same
        // shell as indoor_corridor.rs. No end walls this time: the portals
        // themselves are what closes the hallway off visually.
        let floor = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [HALF_WIDTH, 0.02, HALF_LENGTH]),
        );
        let ceiling = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [HALF_WIDTH, 0.02, HALF_LENGTH]),
        );
        let wall_l = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.02, HALF_HEIGHT, HALF_LENGTH]),
        );
        let wall_r = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.02, HALF_HEIGHT, HALF_LENGTH]),
        );

        // Rooms beyond each end of the corridor, reachable only through the
        // portals: through the near portal you see the far (blue) room, through
        // the far portal the near (amber) room. They sit past the corridor's
        // open ends (z = ±18) so the mapped duplicate never overlaps the real
        // corridor — that overlap is what the fragment z-clip exists to avoid.
        let room_half_len = 6.0;
        let far_room_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.3, 0.55, 0.95, 1.0], 0.7, 0.0, [0.1, 0.5, 1.0], 0.4),
        );
        let near_room_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.95, 0.6, 0.25, 1.0], 0.7, 0.0, [1.0, 0.5, 0.1], 0.4),
        );
        let room_floor = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [HALF_WIDTH, 0.02, room_half_len]),
        );
        let room_wall = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.02, HALF_HEIGHT, room_half_len]),
        );

        // Deliberately *not* INSTANCE_FLAG_ALWAYS_VISIBLE — see portal_cube.rs
        // for why: bounds-based culling (a real center/radius per instance)
        // works fine here too, since every panel's `radius` already covers
        // its own full extent.
        let insert_always = |world: &mut pulsar_scenedb::World,
                             mesh,
                             material,
                             transform: glam::Mat4,
                             radius: f32| {
            let _ = spawn_object(world, mesh, material, transform, radius);
        };
        insert_always(
            &mut scene_db.world,
            floor,
            mat,
            glam::Mat4::IDENTITY,
            HALF_LENGTH,
        );
        insert_always(
            &mut scene_db.world,
            ceiling,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.0 * HALF_HEIGHT, 0.0)),
            HALF_LENGTH,
        );
        insert_always(
            &mut scene_db.world,
            wall_l,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-HALF_WIDTH, HALF_HEIGHT, 0.0)),
            HALF_LENGTH,
        );
        insert_always(
            &mut scene_db.world,
            wall_r,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(HALF_WIDTH, HALF_HEIGHT, 0.0)),
            HALF_LENGTH,
        );
        for (room_mat, zc) in [
            (far_room_mat, -(HALF_LENGTH + room_half_len)),
            (near_room_mat, HALF_LENGTH + room_half_len),
        ] {
            insert_always(
                &mut scene_db.world,
                room_floor,
                room_mat,
                glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, zc)),
                room_half_len,
            );
            insert_always(
                &mut scene_db.world,
                room_floor,
                room_mat,
                glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.0 * HALF_HEIGHT, zc)),
                room_half_len,
            );
            insert_always(
                &mut scene_db.world,
                room_wall,
                room_mat,
                glam::Mat4::from_translation(glam::Vec3::new(-HALF_WIDTH, HALF_HEIGHT, zc)),
                room_half_len,
            );
            insert_always(
                &mut scene_db.world,
                room_wall,
                room_mat,
                glam::Mat4::from_translation(glam::Vec3::new(HALF_WIDTH, HALF_HEIGHT, zc)),
                room_half_len,
            );
        }

        // Evenly spaced ceiling lights so there's plenty of detail for the
        // portal duplicate to actually show — a dark, featureless hallway
        // would look "black" through the portal for an unrelated reason
        // (nothing to light) and defeat the point of the demo.
        let mut light_ids = Vec::new();
        for &z in &[-15.0f32, -9.0, -3.0, 3.0, 9.0, 15.0] {
            light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light([0.0, 2.7, z], [0.9, 0.95, 1.0], 3.0, 7.0),
            ));
        }
        // Colour-coded markers at each end so it's obvious at a glance which
        // end you're looking at through a portal: warm amber in the +Z room,
        // cool blue in the -Z room.
        let near_mat = spawn_material(
            &mut scene_db.world,
            make_material([1.0, 0.6, 0.2, 1.0], 0.6, 0.0, [1.0, 0.5, 0.1], 2.0),
        );
        let far_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.2, 0.6, 1.0, 1.0], 0.6, 0.0, [0.1, 0.5, 1.0], 2.0),
        );
        let marker_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.3, 0.3, 0.05]),
        );
        insert_always(
            &mut scene_db.world,
            marker_mesh,
            near_mat,
            glam::Mat4::from_translation(glam::Vec3::new(
                0.0,
                HALF_HEIGHT,
                HALF_LENGTH + room_half_len,
            )),
            0.5,
        );
        insert_always(
            &mut scene_db.world,
            marker_mesh,
            far_mat,
            glam::Mat4::from_translation(glam::Vec3::new(
                0.0,
                HALF_HEIGHT,
                -(HALF_LENGTH + room_half_len),
            )),
            0.5,
        );
        light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light(
                [0.0, HALF_HEIGHT, HALF_LENGTH + room_half_len],
                [1.0, 0.6, 0.2],
                2.5,
                4.0,
            ),
        ));
        light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light(
                [0.0, HALF_HEIGHT, -(HALF_LENGTH + room_half_len)],
                [0.2, 0.6, 1.0],
                2.5,
                4.0,
            ),
        ));
        // Room lighting so the mapped duplicate is fully lit at its mapped
        // position (the deferred pass lights it where it's drawn, z ≈ ±24).
        for &z in &[-27.0, -21.0, 21.0, 27.0] {
            light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light([0.0, 2.7, z], [0.9, 0.95, 1.0], 3.0, 7.0),
            ));
        }

        // ── Portals ───────────────────────────────────────────────────────
        // Two surfaces: `pose_near` at the +Z end facing further +Z (out
        // through the end of the hall), `pose_far` at the -Z end facing
        // further -Z. Each portal registration duplicates content near one
        // surface so it appears through the other — see the module docs.
        let pose_near = portal_pose_facing(
            glam::Vec3::new(0.0, HALF_HEIGHT, PORTAL_Z),
            glam::Vec3::new(0.0, 0.0, 1.0),
            glam::Vec3::Y,
        );
        let pose_far = portal_pose_facing(
            glam::Vec3::new(0.0, HALF_HEIGHT, -PORTAL_Z),
            glam::Vec3::new(0.0, 0.0, -1.0),
            glam::Vec3::Y,
        );
        let half_extent = glam::Vec2::new(HALF_WIDTH, HALF_HEIGHT);

        // Looking through the near portal (standing near +Z, facing further
        // +Z) shows what's actually near the far end.
        let portal_near = helio_pass_portal_cull::PortalPair {
            a: pose_near,
            b: pose_far,
        };
        // Looking through the far portal shows what's actually near the near
        // end — the reverse direction, completing the loop.
        let portal_far = helio_pass_portal_cull::PortalPair {
            a: pose_far,
            b: pose_near,
        };

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.85, 0.9, 1.0],
            intensity: 0.04,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        self.state = Some(AppState {
            window,
            surface,
            capture: common::SurfaceCapture::from_env(),
            device,
            queue,
            surface_format: format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 1.6, 0.0),
            cam_yaw: 0.0,
            cam_pitch: 0.0,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            portal_near,
            portal_far,
            _light_ids: light_ids,
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
        const SPEED: f32 = 5.0;
        const SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.4, 1.4);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let mut forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);

        let prev_pos = self.cam_pos;
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

        // ── Teleport on crossing either portal ──────────────────────────────
        // Same `helio_pass_portal_cull` math the renderer's own duplicate-content
        // mapping is built on (re-exported from `helio`) — crossing detection
        // and the position/direction remap are just the CPU-side half of the
        // same portal, unaffected by how it's drawn.
        if let Some(pair) = Some(self.portal_near) {
            if helio_pass_portal_cull::crossing_detected(
                prev_pos,
                self.cam_pos,
                &pair.a,
                glam::Vec2::new(HALF_WIDTH, HALF_HEIGHT),
            ) {
                let (new_pos, new_fwd) = pair.teleport_ray(self.cam_pos, forward);
                self.cam_pos = new_pos;
                forward = new_fwd;
                // Inverse of `forward = (sin(yaw)*cos(pitch), sin(pitch), -cos(yaw)*cos(pitch))`.
                self.cam_yaw = forward.x.atan2(-forward.z);
                self.cam_pitch = forward.y.clamp(-1.0, 1.0).asin();
            }
        }
        if let Some(pair) = Some(self.portal_far) {
            if helio_pass_portal_cull::crossing_detected(
                prev_pos,
                self.cam_pos,
                &pair.a,
                glam::Vec2::new(HALF_WIDTH, HALF_HEIGHT),
            ) {
                let (new_pos, new_fwd) = pair.teleport_ray(self.cam_pos, forward);
                self.cam_pos = new_pos;
                forward = new_fwd;
                self.cam_yaw = forward.x.atan2(-forward.z);
                self.cam_pitch = forward.y.clamp(-1.0, 1.0).asin();
            }
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
            100.0,
        );

        common::flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self
            .capture
            .render_window_frame(&mut self.renderer, &camera, &self.surface)
        {
            log::error!("Render error: {:?}", e);
        }
    }
}
