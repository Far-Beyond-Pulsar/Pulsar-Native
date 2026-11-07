//! Infinite tunnel demo — a corridor that repeats forever, but *only through
//! the portals*.
//!
//! One corridor segment (floor, ceiling, side walls, emissive ceiling strip
//! and trim posts) is authored once, then inserted *again* per copy and
//! registered as a **sublevel** placed 500 m below the ground — out of the
//! main pass's frustum, so the corridor reads short from every angle that
//! isn't through a portal.
//!
//! Each portal pairs its real surface (`a`, at a corridor end) with a *remote*
//! pose (`b`) translated straight down by the same 500 m. That makes the
//! portal's coordinate space (`pair_map_inverse`, what
//! `helio-pass-portal-cull` maps content through) a pure vertical shift: the
//! buried copies get pulled up into the corridor's continuation and drawn,
//! clipped in the fragment shader to the portal's opening. They are selected
//! only when their *mapped* position is in the camera's frustum — i.e. when
//! you are actually looking through the portal — and discarded by the clip the
//! moment the mapped position strays off the corridor line.
//!
//! Walking through either portal teleports you to the other end (the corridor
//! repeats every 16 m, so it looks seamless), and you can keep walking forever.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Tab         — toggle editor mode (on by default): shows a checkerboard
//!                 over each portal opening so you can see where it actually
//!                 is; off is the seamless, invisible-portal game-mode look.
//!   Escape      — release cursor / exit

mod common;

use common::PortalComponent;
use common::{
    box_mesh, flush_scene_db, make_material, new_scene_db_with_gpu_mirror, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_pass_portal_cull::{PortalProjectionBridge, SubLevelContents, SubLevelResolver};
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

/// Corridor cross-section half-extent (matches the segment geometry below) —
/// also the portal clip opening's half-extent.
const HALF_WIDTH: f32 = 2.0;
const HALF_HEIGHT: f32 = 1.5;
/// Half-length of one corridor segment; copies sit 2× this apart.
const HALF_LENGTH: f32 = 8.0;
/// Centre-Z of the first copy beyond each end of the central segment.
const COPY_STRIDE: f32 = 2.0 * HALF_LENGTH;
/// Number of sublevel copies per direction.
const COPIES: i32 = 12;
/// Portal surfaces sit exactly at the corridor ends — flush with the real
/// wall/floor/ceiling meshes' own endpoint. Pulling this inward (it used to
/// be `HALF_LENGTH - 1.0`) leaves a strip of real backing geometry between
/// the portal plane and the tunnel's physical end; depending on view angle
/// that strip's depth competes unpredictably with the portal's own mask
/// stamp, which reads as a visible dark border around the opening where the
/// mask loses that fight. Flush placement removes the competing geometry
/// entirely, so the duplicated content lines up seamlessly with the real
/// corridor right up to the frame.
const PORTAL_Z: f32 = HALF_LENGTH;
/// Copies are buried this far below the corridor so the main pass (frustum +
/// far plane) never draws them; the portal's `b` pose is translated down by
/// the same amount, making `pair_map_inverse` a pure +Y shift that pulls the
/// copies up into the corridor's continuation.
const HIDE_OFFSET: f32 = 500.0;
/// Far plane — kept beyond the last continuation copy so the tunnel never
/// visibly ends.
const FAR_PLANE: f32 = 200.0;

/// First group index reserved for the copy sublevels.
const COPY_GROUP_BASE: u8 = 10;

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
    scene_db: SceneDb,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    portal_near: helio_pass_portal_cull::PortalPair,
    portal_far: helio_pass_portal_cull::PortalPair,

    _light_ids: Vec<Entity>,

    /// Debug-only: when `TUNNEL_SCREENSHOT` is set, counts frames so a single
    /// PNG can be captured after the scene has settled, then the process exits.
    frame_count: u32,
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
                        .with_title("Helio – Infinite Tunnel")
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
                application_name: "Infinite Tunnel".into(),
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
            .with_external_device()
            .with_graph(common::preset())
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        // ── Shared geometry & materials for every segment ─────────────────────
        let wall_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.72, 0.72, 0.75, 1.0], 0.8, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let strip_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.95, 0.85, 0.55, 1.0], 0.6, 0.0, [1.0, 0.7, 0.3], 2.5),
        );
        let post_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.35, 0.8, 1.0, 1.0], 0.5, 0.0, [0.2, 0.7, 1.0], 1.5),
        );
        let frame_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.25, 0.95, 1.0, 1.0], 0.4, 0.0, [0.2, 0.9, 1.0], 3.0),
        );

        // Meshes are inserted once and shared by every copy's instances (the
        // copies batch into the same draw calls by mesh+material).
        let slab_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [HALF_WIDTH, 0.02, HALF_LENGTH]),
        );
        let side_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.02, HALF_HEIGHT, HALF_LENGTH]),
        );
        let strip_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.06, 0.02, HALF_LENGTH]),
        );
        let post_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.08, 0.8, 0.08]),
        );
        let frame_box_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.12, 0.12, 0.12]),
        );

        // Central corridor segment (world space, no sublevel).
        insert_segment(
            &mut scene_db.world,
            slab_mesh,
            side_mesh,
            strip_mesh,
            post_mesh,
            wall_mat,
            strip_mat,
            post_mat,
            glam::Mat4::IDENTITY,
        );

        // ── Copy segments as buried sublevels, ±16 m apart, out to ±160 m ──
        // Each copy re-inserts the same objects (shared meshes/materials) with
        // its own group tag, then that group becomes one sublevel whose only
        // per-frame cost is a single coordinate-space slot write. A group's
        // members can hold only one coordinate space (instance flags bits
        // 8-15), which is why each (copy, direction) gets its own group
        // rather than one shared group re-placed.
        //
        // The placement is HIDE_OFFSET below the corridor: from inside the
        // tunnel the main pass culls every copy (below the frustum's far
        // plane), so the corridor looks short. The portal passes pull them
        // back up into the continuation — see the portal component projection below.
        let mut coord_slots = 2u32; // the two portals below
        for sign in [-1.0f32, 1.0] {
            for copy in 1..=COPIES {
                let z = sign * copy as f32 * COPY_STRIDE;
                insert_segment(
                    &mut scene_db.world,
                    slab_mesh,
                    side_mesh,
                    strip_mesh,
                    post_mesh,
                    wall_mat,
                    strip_mat,
                    post_mat,
                    glam::Mat4::from_translation(glam::Vec3::new(0.0, -HIDE_OFFSET, z)),
                );
                coord_slots += 1;
            }
        }
        // 2 portals + 20 sublevels = 22 ≤ MAX_COORDINATE_SPACES (32).
        assert!(
            coord_slots <= 32,
            "coordinate-space budget exceeded: {coord_slots}"
        );

        // ── Portal frames: emissive collars around each opening ───────────────
        // Four thin boxes outline the opening at each end so the portal reads
        // as a framed doorway rather than a bare change in the corridor.
        let frame_t = 0.12; // frame half-thickness
        let mut insert_frame = |cx: f32, cy: f32, cz: f32, hx: f32, hy: f32, hz: f32| {
            let transform = glam::Mat4::from_scale(glam::Vec3::new(hx, hy, hz))
                * glam::Mat4::from_translation(glam::Vec3::new(cx, cy, cz));
            let _ = spawn_object(
                &mut scene_db.world,
                frame_box_mesh,
                frame_mat,
                transform,
                (hx * hx + hy * hy + hz * hz).sqrt(),
            );
        };
        for &z in &[PORTAL_Z, -PORTAL_Z] {
            // Top / bottom rails (full opening width), left / right posts.
            insert_frame(
                0.0,
                HALF_HEIGHT + frame_t,
                z,
                HALF_WIDTH + frame_t,
                frame_t,
                frame_t,
            );
            insert_frame(
                0.0,
                -(HALF_HEIGHT + frame_t),
                z,
                HALF_WIDTH + frame_t,
                frame_t,
                frame_t,
            );
            insert_frame(
                -(HALF_WIDTH + frame_t),
                0.0,
                z,
                frame_t,
                HALF_HEIGHT + frame_t,
                frame_t,
            );
            insert_frame(
                HALF_WIDTH + frame_t,
                0.0,
                z,
                frame_t,
                HALF_HEIGHT + frame_t,
                frame_t,
            );
        }

        // ── Lights down the tunnel so the near copies are lit; the far ones
        // fall off into ambient, which sells the "endless" distance. ─────────
        let mut light_ids = Vec::new();
        for &z in &[0.0f32, -16.0, 16.0, -32.0, 32.0, -48.0, 48.0] {
            light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light([0.0, 2.2, z], [0.9, 0.95, 1.0], 3.0, 12.0),
            ));
        }

        // ── Portals: each pairs its real surface (a, on the corridor end)
        // with a *remote* pose (b) translated HIDE_OFFSET straight down. The
        // portal's coordinate space — `pair_map_inverse`, the map both the
        // cull pass and the duplicate-draw pass apply — becomes a pure +Y
        // shift, so the buried copies are pulled up into the corridor's
        // continuation and clipped to the opening. Because the clip keeps
        // only content on the corridor line past the surface, and the cull
        // selects only content whose *mapped* position is in view, the
        // continuation is visible exactly when you look through the portal —
        // and nowhere else.
        let pose_near = helio_pass_portal_cull::portal_pose_facing(
            glam::Vec3::new(0.0, HALF_HEIGHT, PORTAL_Z),
            glam::Vec3::new(0.0, 0.0, 1.0),
            glam::Vec3::Y,
        );
        let pose_far = helio_pass_portal_cull::portal_pose_facing(
            glam::Vec3::new(0.0, HALF_HEIGHT, -PORTAL_Z),
            glam::Vec3::new(0.0, 0.0, -1.0),
            glam::Vec3::Y,
        );
        // Remote partners: same orientation, HIDE_OFFSET lower. These are
        // what the buried copies' sublevel placements line up with, so the
        // vertical-shift coordinate space maps each copy onto its seat in the
        // tunnel's continuation.
        let pose_near_b = helio_pass_portal_cull::portal_pose_facing(
            glam::Vec3::new(0.0, HALF_HEIGHT - HIDE_OFFSET, PORTAL_Z),
            glam::Vec3::new(0.0, 0.0, 1.0),
            glam::Vec3::Y,
        );
        let pose_far_b = helio_pass_portal_cull::portal_pose_facing(
            glam::Vec3::new(0.0, HALF_HEIGHT - HIDE_OFFSET, -PORTAL_Z),
            glam::Vec3::new(0.0, 0.0, -1.0),
            glam::Vec3::Y,
        );
        let half_extent = glam::Vec2::new(HALF_WIDTH, HALF_HEIGHT);

        // The near (+Z) portal pulls the +Z-direction copies up into the
        // tunnel that continues past the near end; the far (-Z) portal does
        // the same for the -Z direction.
        let portal_near = helio_pass_portal_cull::PortalPair {
            a: pose_near,
            b: pose_near_b,
        };
        let portal_far = helio_pass_portal_cull::PortalPair {
            a: pose_far,
            b: pose_far_b,
        };

        let near_entity = scene_db.world.spawn();
        let far_entity = scene_db.world.spawn();
        scene_db.world.insert(
            near_entity,
            PortalComponent::from(helio_pass_portal_cull::PortalRow::new(
                Some(far_entity),
                pose_near.transform,
                [HALF_WIDTH, HALF_HEIGHT],
            )),
        );
        scene_db.world.insert(
            far_entity,
            PortalComponent::from(helio_pass_portal_cull::PortalRow::new(
                Some(near_entity),
                pose_far.transform,
                [HALF_WIDTH, HALF_HEIGHT],
            )),
        );
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (
                        near_entity,
                        helio_pass_portal_cull::PortalRow::new(
                            Some(far_entity),
                            pose_near.transform,
                            [HALF_WIDTH, HALF_HEIGHT],
                        ),
                    ),
                    (
                        far_entity,
                        helio_pass_portal_cull::PortalRow::new(
                            Some(near_entity),
                            pose_far.transform,
                            [HALF_WIDTH, HALF_HEIGHT],
                        ),
                    ),
                ],
            ),
        );
        let projection = PortalProjectionBridge::new(1)
            .expect("portal projection depth")
            .build(&resolver)
            .expect("portal projection");
        let projection_entities = [near_entity, far_entity];
        common::publish_portal_projection(
            &mut scene_db.world,
            &projection,
            &projection_entities,
            &projection_entities,
            near_entity,
        )
        .expect("portal projection rows");
        common::set_projection_frame(&mut renderer, &projection);

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.85, 0.9, 1.0],
            intensity: 0.05,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        // Editor mode (Tab to toggle) starts on so the portal-opening
        // checkerboard — otherwise invisible, see PortalEditorOverlayPass —
        // is visible by default; press Tab to see the fully seamless game-
        // mode look.
        renderer.set_editor_mode(true);

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 1.6, 0.0),
            cam_yaw: std::f32::consts::PI,
            cam_pitch: 0.0,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            portal_near,
            portal_far,
            _light_ids: light_ids,
            frame_count: 0,
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
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Tab),
                        repeat: false,
                        ..
                    },
                ..
            } => {
                let enabled = !state.renderer.is_editor_mode();
                state.renderer.set_editor_mode(enabled);
                log::info!("[infinite_tunnel] editor mode: {}", enabled);
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
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
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

        // ── Teleport on crossing either portal: the tunnel never ends ────────
        // The render-side portal pair is a *remote* pose (b sits HIDE_OFFSET
        // below ground so the portal's coordinate space is a vertical shift),
        // so `pair.teleport_ray` would drop the player 500 m. Crossing still
        // uses `pair.a` (the real surface at the corridor end); the remap is
        // the corridor's own symmetry map, (-x, y, -z), which sends the near
        // end to the far end and vice versa. Only one teleport per frame.
        let mut teleported = false;
        for portal in [self.portal_near, self.portal_far] {
            if let Some(pair) = Some(portal) {
                if helio_pass_portal_cull::crossing_detected(
                    prev_pos,
                    self.cam_pos,
                    &pair.a,
                    glam::Vec2::new(HALF_WIDTH, HALF_HEIGHT),
                ) {
                    self.cam_pos =
                        glam::Vec3::new(-self.cam_pos.x, self.cam_pos.y, -self.cam_pos.z);
                    forward = glam::Vec3::new(-forward.x, forward.y, -forward.z);
                    self.cam_yaw = forward.x.atan2(-forward.z);
                    self.cam_pitch = forward.y.clamp(-1.0, 1.0).asin();
                    teleported = true;
                    break;
                }
            }
        }
        let _ = teleported;

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            FAR_PLANE,
        );

        flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }

        // ── Debug screenshot path ──────────────────────────────────────────
        // TUNNEL_SCREENSHOT=<path>: after the scene has had a few frames to
        // settle (cull-pass readback etc.), copy the just-rendered composited
        // frame out to a PNG and exit. Avoids needing a human to look at the
        // window — used to verify portal-opening content lands on screen.
        self.frame_count += 1;
        if let Ok(path) = std::env::var("TUNNEL_SCREENSHOT") {
            const CAPTURE_AT_FRAME: u32 = 90;
            if self.frame_count == CAPTURE_AT_FRAME {
                // The swapchain's own texture isn't guaranteed COPY_SRC on
                // every backend, so re-render this frame's camera into an
                // offscreen texture we own (created with COPY_SRC) purely for
                // the capture — one extra render call, only on capture frames.
                let offscreen = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Screenshot Offscreen"),
                    size: wgpu::Extent3d {
                        width: size.width,
                        height: size.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.surface_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let offscreen_view = offscreen.create_view(&Default::default());
                if let Err(e) = self.renderer.render(&camera, &offscreen_view) {
                    log::error!("Screenshot render: {:?}", e);
                }
                capture_screenshot(
                    &self.device,
                    &self.queue,
                    &offscreen,
                    self.surface_format,
                    &path,
                );
                std::process::exit(0);
            }
        }
    }
}

/// Copies `texture` (must have been created/configured with `COPY_SRC`) back
/// to the CPU and writes it out as a PNG. Assumes an 8-bit-per-channel RGBA
/// or BGRA surface format (true for every format the surface capability query
/// in this example can select).
fn capture_screenshot(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    path: &str,
) {
    let width = texture.width();
    let height = texture.height();
    let bytes_per_pixel = 4u32;
    let unpadded_bytes_per_row = width * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Screenshot Readback"),
        size: (padded_bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Screenshot Encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map screenshot buffer"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("get_mapped_range");

    let is_bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut pixels = vec![0u8; (width * height * bytes_per_pixel) as usize];
    for row in 0..height {
        let src_start = (row * padded_bytes_per_row) as usize;
        let src_row = &data[src_start..src_start + unpadded_bytes_per_row as usize];
        let dst_start = (row * unpadded_bytes_per_row) as usize;
        let dst_row = &mut pixels[dst_start..dst_start + unpadded_bytes_per_row as usize];
        dst_row.copy_from_slice(src_row);
        if is_bgra {
            for px in dst_row.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
    }
    drop(data);
    buffer.unmap();

    image::save_buffer(path, &pixels, width, height, image::ColorType::Rgba8)
        .expect("save screenshot png");
    log::info!("[Screenshot] wrote {path} ({width}x{height})");
}

/// Inserts one corridor segment's objects, tagging each into `groups` (an
/// ordinary `GroupMask::NONE` for the central segment, a copy group for the
/// sublevel copies). All transforms are segment-local — the sublevel's
/// coordinate space places a copy, the objects themselves never move.
///
/// The central segment keeps `INSTANCE_FLAG_ALWAYS_VISIBLE` (it is the shell
/// the camera lives inside, and its bounding spheres are large/flat enough
/// that sphere culling is a liability). The buried copies are *not*
/// always-visible: they must be culled by the main pass (they sit below the
/// far plane) and only selected by the portal cull, when their mapped
/// position is actually in view.
fn insert_segment(
    world: &mut pulsar_scenedb::World,
    slab_mesh: Entity,
    side_mesh: Entity,
    strip_mesh: Entity,
    post_mesh: Entity,
    wall_mat: Entity,
    strip_mat: Entity,
    post_mat: Entity,
    placement: glam::Mat4,
) {
    let mut insert = |mesh: Entity, material: Entity, transform: glam::Mat4, radius: f32| {
        let _ = spawn_object(world, mesh, material, placement * transform, radius);
    };
    // Floor + ceiling share the slab mesh; left + right walls share the side mesh.
    insert(slab_mesh, wall_mat, glam::Mat4::IDENTITY, HALF_LENGTH);
    insert(
        slab_mesh,
        wall_mat,
        glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.0 * HALF_HEIGHT, 0.0)),
        HALF_LENGTH,
    );
    insert(
        side_mesh,
        wall_mat,
        glam::Mat4::from_translation(glam::Vec3::new(-HALF_WIDTH, HALF_HEIGHT, 0.0)),
        HALF_LENGTH,
    );
    insert(
        side_mesh,
        wall_mat,
        glam::Mat4::from_translation(glam::Vec3::new(HALF_WIDTH, HALF_HEIGHT, 0.0)),
        HALF_LENGTH,
    );
    // Emissive ceiling strip + trim posts — the periodic detail that makes the
    // repetition of the tunnel obvious at a glance.
    insert(
        strip_mesh,
        strip_mat,
        glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.0 * HALF_HEIGHT - 0.02, 0.0)),
        HALF_LENGTH,
    );
    for &(x, z) in &[
        (-HALF_WIDTH + 0.4, -HALF_LENGTH * 0.5),
        (HALF_WIDTH - 0.4, -HALF_LENGTH * 0.5),
        (-HALF_WIDTH + 0.4, HALF_LENGTH * 0.5),
        (HALF_WIDTH - 0.4, HALF_LENGTH * 0.5),
    ] {
        insert(
            post_mesh,
            post_mat,
            glam::Mat4::from_translation(glam::Vec3::new(x, 0.4, z)),
            0.6,
        );
    }
}
