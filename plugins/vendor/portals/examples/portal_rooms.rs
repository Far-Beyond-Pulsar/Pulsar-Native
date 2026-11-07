//! Portal rooms — a cube with *no walls of its own at all*. Each of its 6
//! faces is nothing but a portal, edge to edge, no doorway cut into a wall
//! and no frame or border around it — the face itself is the entire portal
//! surface. Stand outside it and every face shows a different, fully
//! furnished scene filling it completely — a bedroom through one face, a
//! kitchen through the next, a library, a lounge, a greenhouse, a spa — with
//! nothing marking where the "wall" is, because there isn't one. That's
//! deliberate: without a doorway shape or a frame telling you "this is the
//! portal, right here", it's not obvious at a glance which face is
//! solid-looking-but-isn't, which makes the illusion more disorienting (in a
//! good way) than `portal_cube`'s framed doorways. None of it is faked: each
//! face is a real SceneDB portal components pairing that whole face with the
//! real entrance of a real, hand-furnished room built somewhere else in world
//! space, and the engine's own portal pipeline (`helio-pass-portal-cull` /
//! `helio-pass-portal-instances`) does the rest — the only difference from
//! `portal_cube` is that the portal's `half_extent` covers the *entire* face
//! instead of a doorway inset into a wall, and that wall is never built in
//! the first place.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Tab         — toggle editor mode (on by default): shows a checkerboard
//!                 over each portal opening so you can see where it is.
//!   Escape      — release cursor / exit

mod common;

use common::PortalComponent;
use common::{
    box_mesh, flush_scene_db, make_material, new_scene_db_with_gpu_mirror, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object, sphere_mesh,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use helio_pass_portal_cull::{PortalProjectionBridge, SubLevelContents, SubLevelResolver};
use pulsar_scenedb::{Entity, SceneDb, World};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use glam::{Mat4, Vec3};
use std::collections::HashSet;
use std::sync::Arc;

/// Hub half-extent — there's no hub geometry at all (see the module doc):
/// this is purely the position of each face's portal plane and the
/// half-extent of its clip window, so the portal covers the *entire* face,
/// corner to corner.
const HUB_HALF_SIZE: f32 = 6.0;
/// Side-room wall panel half-thickness.
const WALL_T: f32 = 0.15;

/// Each side room's own half-extent. Deliberately *equal* to `HUB_HALF_SIZE`
/// — not just its depth but its cross-section (width/height) too — so the
/// room's floor, wall, and ceiling edges land exactly on the portal's own
/// window edges instead of continuing past them. A wider-than-the-window
/// room isn't a bug (a window narrower than the room behind it is normal —
/// picture a real window: you don't get to see past its frame), but it does
/// mean the floor's side edges, traced back toward the viewer, visibly stop
/// short of the portal's own corners instead of meeting them, which reads as
/// "something's projected wrong" even though every vertex is real, exact 3D
/// geometry. Equal sizing sidesteps the question entirely: every edge of the
/// room's open face *is* an edge of the portal's own window, by construction.
const ROOM_HALF_SIZE: f32 = HUB_HALF_SIZE;
/// World-space X of the *first* room's center. All six sit in a single
/// straight row along +X from here — see the `rooms` loop in `resumed` for
/// why: it's the plainest possible proof that nothing spatially impossible
/// is going on. Each portal still shows its own room facing whichever
/// direction *that* portal looks, via `pair_map_inverse`'s rotation (not
/// just translation) — but if you fly the camera away from the hub
/// entirely and out to where the rooms actually are, what's really there is
/// just six ordinary, identically-oriented rooms sitting side by side, the
/// same as any row of real rooms in a real building.
const ROOM_LINE_START_X: f32 = 60.0;
/// Center-to-center spacing along the row — `2*ROOM_HALF_SIZE` (the room's
/// own width) plus a clear gap, so neighboring rooms' walls never touch.
const ROOM_LINE_SPACING: f32 = 2.0 * ROOM_HALF_SIZE + 8.0;

/// Far plane — generous enough to cover the farthest room in the row
/// (`ROOM_LINE_START_X + 5*ROOM_LINE_SPACING + ROOM_HALF_SIZE`) plus real
/// flying-around margin.
const FAR_PLANE: f32 = 300.0;

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

    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    _light_ids: Vec<Entity>,

    /// Debug-only: when `ROOMS_SCREENSHOT` is set, counts frames so a single
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
                        .with_title("Helio – Portal Rooms")
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
                application_name: "Portal Rooms".into(),
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

        // Portal GPU rows are entity-indexed. Allocate their six rows before
        // the rest of the scene so the portal table is compact and contains
        // no ambiguity about which chain slots are authored.
        let projection_entities: Vec<Entity> = (0..6).map(|_| scene_db.world.spawn()).collect();
        let authored_portal_entities: Vec<(Entity, Entity)> = (0..6)
            .map(|_| (scene_db.world.spawn(), scene_db.world.spawn()))
            .collect();

        // Two shared unit meshes (half-extent/radius 1) — every side room's
        // shell panel, piece of furniture, and accent prop in this scene is
        // one of these two, scaled/positioned per instance via its own
        // transform (see `insert_room_shell`/`furnish_room` below). The hub
        // itself has no geometry — see the module doc.
        let unit_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
        );
        let unit_sphere = spawn_mesh(&mut scene_db.world, sphere_mesh([0.0, 0.0, 0.0], 1.0));

        // Shared furniture materials, reused across every room so the six
        // spaces read as built from the same "kit" — only each room's own
        // wall/accent colors (below) tell them apart.
        let wood_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.32, 0.2, 0.11, 1.0], 0.75, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let metal_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.5, 0.51, 0.54, 1.0], 0.4, 0.6, [0.0, 0.0, 0.0], 0.0),
        );

        // ── The hub: one full-face portal per axis direction, no wall, no
        // doorway cutout, no frame. `up_hint` just needs to not be parallel
        // to `normal` — Y for the four side faces, Z for the top/bottom
        // (where normal itself is ±Y). The orthonormalized (right, up,
        // normal) triple this produces is the portal's own pose basis below.
        struct RoomTheme {
            name: &'static str,
            wall_color: [f32; 4],
            accent: [f32; 3],
        }
        let faces: [(Vec3, Vec3, RoomTheme); 6] = [
            (
                Vec3::X,
                Vec3::Y,
                RoomTheme {
                    name: "Ember",
                    wall_color: [0.55, 0.12, 0.08, 1.0],
                    accent: [1.0, 0.35, 0.1],
                },
            ),
            (
                Vec3::NEG_X,
                Vec3::Y,
                RoomTheme {
                    name: "Verdant",
                    wall_color: [0.08, 0.4, 0.14, 1.0],
                    accent: [0.25, 1.0, 0.35],
                },
            ),
            (
                Vec3::Y,
                Vec3::Z,
                RoomTheme {
                    name: "Solar",
                    wall_color: [0.55, 0.45, 0.05, 1.0],
                    accent: [1.0, 0.85, 0.15],
                },
            ),
            (
                Vec3::NEG_Y,
                Vec3::Z,
                RoomTheme {
                    name: "Abyssal",
                    wall_color: [0.05, 0.1, 0.4, 1.0],
                    accent: [0.2, 0.4, 1.0],
                },
            ),
            (
                Vec3::Z,
                Vec3::Y,
                RoomTheme {
                    name: "Orchid",
                    wall_color: [0.42, 0.08, 0.48, 1.0],
                    accent: [0.9, 0.25, 1.0],
                },
            ),
            (
                Vec3::NEG_Z,
                Vec3::Y,
                RoomTheme {
                    name: "Glacier",
                    wall_color: [0.08, 0.4, 0.45, 1.0],
                    accent: [0.2, 0.95, 1.0],
                },
            ),
        ];

        // Every real room shares one fixed orientation, regardless of which
        // cube face its own portal happens to be on — `ROOM_RIGHT`/`ROOM_UP`
        // for its own walls and furniture, entrance facing `-ROOM_FORWARD`
        // (the wall skipped by `insert_room_shell`'s `open_normal` below),
        // interior extending along `+ROOM_FORWARD` from there. An ordinary
        // room built the ordinary way — nothing about its own construction
        // references any portal at all.
        const ROOM_RIGHT: Vec3 = Vec3::X;
        const ROOM_UP: Vec3 = Vec3::Y;
        const ROOM_FORWARD: Vec3 = Vec3::Z;

        let mut authored_portals = Vec::with_capacity(faces.len() * 2);
        let mut light_ids = Vec::new();
        for (i, (normal, up_hint, theme)) in faces.iter().enumerate() {
            let normal = *normal;
            let up_hint = *up_hint;
            let right = up_hint.cross(normal).normalize();
            let up = normal.cross(right).normalize();

            // The destination room for this face is just the i'th room in
            // the straight row — see `ROOM_LINE_START_X`'s doc for why. Its
            // position has nothing to do with `normal` at all; only the
            // *portal* (`a`, below) needs to know which cube face it's on.
            let room_center = Vec3::new(ROOM_LINE_START_X + i as f32 * ROOM_LINE_SPACING, 0.0, 0.0);
            let room_wall_mat = spawn_material(
                &mut scene_db.world,
                make_material(theme.wall_color, 0.85, 0.0, [0.0, 0.0, 0.0], 0.0),
            );
            let room_accent_mat = spawn_material(
                &mut scene_db.world,
                make_material(
                    [theme.accent[0], theme.accent[1], theme.accent[2], 1.0],
                    0.3,
                    0.0,
                    theme.accent,
                    3.0,
                ),
            );
            // Leave the entrance wall (`-ROOM_FORWARD`) open — that's the
            // room's real entrance, the same real surface the portal's far
            // pose sits at, so there's real geometry (floor, ceiling, far
            // wall, side walls) waiting right where the doorway leads.
            insert_room_shell(
                &mut scene_db.world,
                unit_mesh,
                room_wall_mat,
                room_center,
                ROOM_HALF_SIZE,
                WALL_T,
                -ROOM_FORWARD,
            );
            // Hand-furnish this room so each destination reads as an actual
            // place, not just a colored box — see `furnish_room` for the
            // per-theme layouts — plus a matching light so the room isn't
            // lit solely by the hub's distant, unrelated lights.
            let frame = RoomFrame {
                center: room_center,
                right: ROOM_RIGHT,
                up: ROOM_UP,
                normal: -ROOM_FORWARD,
                half_size: ROOM_HALF_SIZE,
            };
            furnish_room(
                &mut scene_db.world,
                unit_mesh,
                unit_sphere,
                wood_mat,
                metal_mat,
                room_wall_mat,
                room_accent_mat,
                theme.name,
                &frame,
            );
            light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light(room_center.into(), theme.accent, 3.5, ROOM_HALF_SIZE * 1.8),
            ));
            log::info!(
                "[portal_rooms] {} room centered at {:?}",
                theme.name,
                room_center
            );

            // Pair the hub's whole face (`a`, forced to face *into* the
            // cube from outside, same as before) with this room's real
            // entrance (`b`, at the room's own fixed orientation — *not*
            // rotated to match `a`). `pair_map_inverse` is a full rigid
            // transform, not just a translation, so when `a` and `b` face
            // different ways (true for 5 of these 6 — only the one portal
            // whose own `-normal` happens to equal `ROOM_FORWARD` lines up
            // by coincidence) the room is duplicated *rotated* to match
            // what's actually on the other side of that specific face —
            // exactly what a real portal connecting two independently-built
            // spaces should do, and proof this was never a translation-only
            // special case: an ordinary room, built with no reference to
            // any portal, still shows through correctly regardless of which
            // way its own portal happens to be facing. `half_extent` covers
            // the entire face — full corner-to-corner coverage, no doorway
            // inset — which is what makes the face itself *be* the portal
            // instead of a hole cut into a wall.
            let a = helio_pass_portal_cull::portal_pose_facing(normal * HUB_HALF_SIZE, normal, up);
            let entrance = room_center - ROOM_FORWARD * ROOM_HALF_SIZE;
            let b = helio_pass_portal_cull::portal_pose_facing(entrance, ROOM_FORWARD, ROOM_UP);
            let (source_entity, target_entity) = authored_portal_entities[i];
            let source = PortalComponent::from(helio_pass_portal_cull::PortalRow::new(
                Some(target_entity),
                a.transform,
                [HUB_HALF_SIZE, HUB_HALF_SIZE],
            ));
            let mut target = PortalComponent::from(helio_pass_portal_cull::PortalRow::new(
                Some(source_entity),
                b.transform,
                [ROOM_HALF_SIZE, ROOM_HALF_SIZE],
            ));
            // The room entrance is the peer/content anchor for this demo,
            // not another visible portal opening. It remains authored and
            // peer-linked so the generalized resolver can resolve it.
            target.flags &= !helio_pass_portal_cull::PortalRow::FLAG_ENABLED;
            scene_db.world.insert(source_entity, source);
            scene_db.world.insert(target_entity, target);
            authored_portals.push((source_entity, source));
            authored_portals.push((target_entity, target));
        }

        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new([], authored_portals.iter().map(|(e, p)| (*e, p.row()))),
        );
        let projection_frame = PortalProjectionBridge::new(1)
            .expect("portal_rooms projection depth")
            .build(&resolver)
            .expect("portal_rooms authored portal graph");
        common::publish_portal_projection(
            &mut scene_db.world,
            &projection_frame,
            &projection_entities,
            &projection_entities,
            projection_entities[0],
        )
        .expect("portal_rooms dense projection rows");
        common::set_projection_frame(&mut renderer, &projection_frame);

        // ── A light near the hub's center so its own walls read clearly.
        light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light(
                [0.0, HUB_HALF_SIZE * 0.85, 0.0],
                [1.0, 0.98, 0.92],
                4.0,
                HUB_HALF_SIZE * 1.8,
            ),
        ));

        // Deferred lighting shades every pixel — including portal
        // duplicates — by real distance to the scene's real lights. Each
        // room has its own themed light for that reason, but ambient is
        // still pushed up well past a typical scene's default so every room
        // reads clearly regardless of camera position, same as `portal_cube`.
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.5, 0.52, 0.58],
            intensity: 0.3,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        // Editor mode (Tab to toggle) starts on so the portal-opening
        // checkerboard — otherwise invisible, see PortalEditorOverlayPass —
        // is visible by default; press Tab to see the fully seamless game-
        // mode look.
        renderer.set_editor_mode(std::env::var("ROOMS_NO_EDITOR").is_err());

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            scene_db,
            last_frame: std::time::Instant::now(),
            // Off the centerline of every doorway, near a corner, so
            // several different faces are all in view — and none of them is
            // seen exactly head-on, which for a pure-translation portal pair
            // would just show a flat, undistorted view straight down its
            // axis (see `portal_cube`'s own comment on this). Well outside
            // the cube (HUB_HALF_SIZE=6) so the whole thing is in frame,
            // since every face is now the portal itself with nothing to
            // stand inside — see the module doc.
            // `ROOMS_ROW_VIEW=1` starts looking at the real room row instead
            // of the hub — the plain, ordinary-looking line of six rooms
            // that every portal above actually connects to. Useful for
            // convincing yourself nothing spatially impossible is going on:
            // fly out here and it's just six boxes in a row, like any real
            // building.
            cam_pos: if std::env::var("ROOMS_ROW_VIEW").is_ok() {
                Vec3::new(110.0, 15.0, -50.0)
            } else {
                Vec3::new(16.0, 12.0, 16.0)
            },
            cam_yaw: if std::env::var("ROOMS_ROW_VIEW").is_ok() {
                std::f32::consts::PI
            } else {
                -0.785
            },
            cam_pitch: if std::env::var("ROOMS_ROW_VIEW").is_ok() {
                -0.25
            } else {
                -0.488
            },
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
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
                log::info!("[portal_rooms] editor mode: {}", enabled);
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
        const SPEED: f32 = 4.0;
        const SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.4, 1.4);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = Vec3::new(sy * cp, sp, -cy * cp);
        let right = Vec3::new(cy, 0.0, sy);

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
            self.cam_pos += Vec3::Y * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos -= Vec3::Y * SPEED * dt;
        }

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            FAR_PLANE,
        );

        // Publish authoritative SceneDB component rows before the render graph
        // consumes the GPU mirror.
        flush_scene_db(&self.scene_db, &self.queue);

        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }

        // ── Debug screenshot path — see infinite_tunnel.rs for the full
        // rationale (no way to drive a native window interactively here).
        self.frame_count += 1;
        if let Ok(path) = std::env::var("ROOMS_SCREENSHOT") {
            const CAPTURE_AT_FRAME: u32 = 90;
            if self.frame_count == CAPTURE_AT_FRAME {
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

/// Inserts a single axis-aligned box panel (the shared unit mesh scaled to
/// `half_extent` and moved to `center`) — the shared building block for
/// every side room's shell walls plus its floating accent prop.
fn insert_box_panel(
    world: &mut World,
    unit_mesh: Entity,
    material: Entity,
    center: Vec3,
    half_extent: Vec3,
) {
    let transform = Mat4::from_cols(
        (Vec3::X * half_extent.x).extend(0.0),
        (Vec3::Y * half_extent.y).extend(0.0),
        (Vec3::Z * half_extent.z).extend(0.0),
        center.extend(1.0),
    );
    let radius = half_extent.length();
    if let Ok(entity) = spawn_object(world, unit_mesh, material, transform, radius) {
        // Debug mode: keep every target mesh in the portal scene alive
        // through frustum and Hi-Z culling so portal failures are not
        // confused with ordinary scene culling failures.
        if let Some(mut object) = world.get_mut::<helio_mesh_host::StaticObjectComponent>(entity) {
            object.flags |= helio_mesh_api::INSTANCE_FLAG_ALWAYS_VISIBLE;
        }
    }
}

/// Inserts a side room's shell: a `half_size`-cube built from 6 axis-aligned
/// wall panels of thickness `wall_t`, skipping whichever face's outward
/// normal matches `open_normal` (within tolerance) — that's the room's real
/// "entrance", the exact surface a portal's far pose sits at, so it's left
/// open rather than sealed. Leaving it open (instead of giving it its own
/// doorway hole, the way `portal_cube`'s walls do) avoids a wall panel
/// sitting exactly on the portal's own clip plane, which is unnecessary
/// here since — unlike `portal_cube` — nothing needs to recurse back out of
/// these terminal, single-destination rooms.
fn insert_room_shell(
    world: &mut World,
    unit_mesh: Entity,
    wall_mat: Entity,
    center: Vec3,
    half_size: f32,
    wall_t: f32,
    open_normal: Vec3,
) {
    let panels: [(Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::new(wall_t, half_size, half_size)),
        (Vec3::NEG_X, Vec3::new(wall_t, half_size, half_size)),
        (Vec3::Y, Vec3::new(half_size, wall_t, half_size)),
        (Vec3::NEG_Y, Vec3::new(half_size, wall_t, half_size)),
        (Vec3::Z, Vec3::new(half_size, half_size, wall_t)),
        (Vec3::NEG_Z, Vec3::new(half_size, half_size, wall_t)),
    ];
    for (dir, half_extent) in panels {
        if dir.dot(open_normal) > 0.5 {
            continue;
        }
        insert_box_panel(
            world,
            unit_mesh,
            wall_mat,
            center + dir * half_size,
            half_extent,
        );
    }
}

/// A room's own local placement frame — furniture below is authored once, in
/// this frame's `(rx, height, depth)` coordinates (`rx` left/right from the
/// room's own centerline, `height` above its floor, `depth` into the room
/// from its open entrance wall), and converted to world space by `point`/
/// `extent`. That's what lets the same six-line bedroom layout, say, work
/// unchanged regardless of whether that particular room hangs off +X or +Y —
/// only `right`/`up`/`normal` (and thus what world axis "rx"/"height"/"depth"
/// actually move along) differ per room.
struct RoomFrame {
    center: Vec3,
    right: Vec3,
    up: Vec3,
    normal: Vec3,
    half_size: f32,
}

impl RoomFrame {
    /// World position for a piece centered at `rx` (right axis, 0 = room's
    /// own centerline), `height` above the floor, and `depth` into the room
    /// from its open entrance wall (0 = entrance, `2*half_size` = back wall).
    fn point(&self, rx: f32, height: f32, depth: f32) -> Vec3 {
        self.center
            + self.right * rx
            + self.up * (height - self.half_size)
            + self.normal * (self.half_size - depth)
    }

    /// World-space half-extent for a piece whose half-size is `hr`/`hu`/`hn`
    /// along this room's own right/up/normal axes. `abs()` is safe here
    /// (not a general basis transform) because, for every one of this
    /// scene's 6 cube faces, `right`/`up`/`normal` are each exactly ± one of
    /// the world axes — see the `faces` table in `resumed`.
    fn extent(&self, hr: f32, hu: f32, hn: f32) -> Vec3 {
        (self.right * hr + self.up * hu + self.normal * hn).abs()
    }
}

/// Places one furniture piece (the shared `mesh` — `unit_mesh` for boxes,
/// `unit_sphere` for rounded pieces — scaled to `(hr, hu, hn)` and moved to
/// `(rx, height, depth)`) in `frame`'s local coordinates. See `RoomFrame`.
#[allow(clippy::too_many_arguments)]
fn place(
    world: &mut World,
    mesh: Entity,
    material: Entity,
    frame: &RoomFrame,
    rx: f32,
    height: f32,
    depth: f32,
    hr: f32,
    hu: f32,
    hn: f32,
) {
    insert_box_panel(
        world,
        mesh,
        material,
        frame.point(rx, height, depth),
        frame.extent(hr, hu, hn),
    );
}

/// Hand-furnishes one side room so it reads as an actual place instead of a
/// bare box — six different layouts, one per `RoomTheme::name`, all built
/// from the same small kit of shared meshes/materials (`unit_mesh`/
/// `unit_sphere` for shape, `wood_mat`/`metal_mat` for hard furniture,
/// `base_mat`/`accent_mat` — this room's own muted/emissive theme colors —
/// for soft furnishings and lamps/glow respectively). Every position below
/// is authored in `frame`'s local `(rx, height, depth)` coordinates, so it
/// reads the same regardless of which world axis this particular room's
/// portal actually points along.
#[allow(clippy::too_many_arguments)]
fn furnish_room(
    world: &mut World,
    unit_mesh: Entity,
    unit_sphere: Entity,
    wood_mat: Entity,
    metal_mat: Entity,
    base_mat: Entity,
    accent_mat: Entity,
    name: &str,
    frame: &RoomFrame,
) {
    let b = |w: &mut World, mat, rx, h, d, hr, hu, hn| {
        place(w, unit_mesh, mat, frame, rx, h, d, hr, hu, hn)
    };
    let s = |w: &mut World, mat, rx, h, d, radius: f32| {
        place(w, unit_sphere, mat, frame, rx, h, d, radius, radius, radius)
    };

    match name {
        "Ember" => {
            // Bedroom: bed against the back wall, nightstand + lamp beside
            // it, a wardrobe near the entrance, a rug underfoot.
            b(world, wood_mat, -1.5, 0.7, 8.5, 2.4, 0.7, 3.0); // bed frame
            b(world, base_mat, -1.5, 1.5, 8.5, 2.2, 0.3, 2.8); // mattress
            b(world, base_mat, -1.5, 1.9, 10.3, 1.0, 0.25, 0.7); // pillow
            b(world, wood_mat, 1.5, 0.5, 9.5, 0.6, 0.5, 0.6); // nightstand
            s(world, accent_mat, 1.5, 1.4, 9.5, 0.4); // lamp
            b(world, wood_mat, -4.8, 1.8, 2.2, 0.8, 1.8, 1.0); // wardrobe
            b(world, base_mat, -1.0, 0.04, 6.0, 2.6, 0.04, 3.2); // rug
        }
        "Verdant" => {
            // Greenhouse: a potting table, three planters with plants along
            // the back wall, a bench near the entrance.
            b(world, wood_mat, 0.0, 0.5, 5.0, 1.8, 0.5, 0.9); // potting table
            for rx in [-2.5, 0.0, 2.5] {
                b(world, wood_mat, rx, 0.5, 10.5, 0.6, 0.5, 0.6); // planter
                s(world, accent_mat, rx, 1.4, 10.5, 0.55); // plant
            }
            b(world, wood_mat, -4.5, 0.4, 2.0, 0.5, 0.4, 1.8); // bench
        }
        "Solar" => {
            // Kitchen: counter + stove along the back wall, a small table
            // and two chairs, a light fixture overhead.
            b(world, metal_mat, 0.0, 0.7, 10.5, 4.5, 0.7, 0.8); // counter
            b(world, metal_mat, -1.5, 1.55, 10.5, 0.8, 0.15, 0.6); // stove
            s(world, accent_mat, -1.8, 1.72, 10.5, 0.16); // burner
            s(world, accent_mat, -1.2, 1.72, 10.5, 0.16); // burner
            b(world, wood_mat, 0.0, 0.75, 5.0, 1.5, 0.1, 1.5); // table
            b(world, wood_mat, -2.0, 0.4, 5.0, 0.45, 0.4, 0.45); // chair
            b(world, wood_mat, 2.0, 0.4, 5.0, 0.45, 0.4, 0.45); // chair
            s(world, accent_mat, 0.0, 11.0, 6.0, 0.5); // light fixture
        }
        "Abyssal" => {
            // Library: a full bookshelf on the back wall with a row of
            // colored "books", a desk, a chair, a reading lamp.
            b(world, wood_mat, 0.0, 3.0, 11.3, 4.5, 3.0, 0.5); // bookshelf
            for (i, rx) in [-3.0, -1.5, 0.0, 1.5, 3.0].into_iter().enumerate() {
                let mat = if i % 2 == 0 { accent_mat } else { base_mat };
                b(world, mat, rx, 4.2, 11.0, 0.4, 0.9, 0.15); // books
            }
            b(world, wood_mat, 0.0, 0.75, 4.5, 1.6, 0.1, 0.9); // desk
            b(world, base_mat, 0.0, 0.45, 3.0, 0.5, 0.45, 0.5); // chair
            s(world, accent_mat, 1.2, 1.5, 4.5, 0.3); // desk lamp
        }
        "Orchid" => {
            // Lounge: sofa facing a glowing "screen" on the back wall, a
            // coffee table, a floor lamp.
            b(world, base_mat, 0.0, 0.45, 9.5, 2.6, 0.45, 1.0); // sofa base
            b(world, base_mat, 0.0, 1.2, 10.4, 2.6, 0.5, 0.25); // sofa back
            b(world, accent_mat, 0.0, 3.4, 11.7, 1.8, 1.0, 0.12); // screen
            b(world, wood_mat, 0.0, 0.4, 6.5, 1.2, 0.15, 0.7); // coffee table
            b(world, metal_mat, -4.5, 1.9, 4.5, 0.1, 1.9, 0.1); // lamp pole
            s(world, accent_mat, -4.5, 4.0, 4.5, 0.5); // lamp shade
        }
        "Glacier" => {
            // Spa: a tub, a sink counter with a glowing mirror above it, a
            // stool, a couple of loose accent "ice" pieces.
            b(world, metal_mat, 0.0, 0.55, 8.5, 1.8, 0.55, 1.3); // tub
            b(world, metal_mat, -4.0, 0.75, 2.5, 1.0, 0.1, 0.7); // sink counter
            b(world, accent_mat, -4.0, 2.0, 11.7, 0.8, 0.9, 0.12); // mirror
            b(world, wood_mat, 3.0, 0.35, 3.5, 0.4, 0.35, 0.4); // stool
            s(world, accent_mat, 3.5, 1.1, 7.0, 0.4); // accent
            s(world, accent_mat, -3.5, 1.1, 8.0, 0.35); // accent
        }
        _ => {}
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
