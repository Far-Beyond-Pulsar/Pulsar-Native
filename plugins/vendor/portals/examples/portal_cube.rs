//! Portal cube — a sealed room with a doorway-shaped portal in the center of
//! each of its 6 walls, each one reflecting the *same real room* back at
//! itself. No manually-authored "copies" anywhere in this file: every
//! reflection you see — including the second, third bounce receding into
//! each doorway — comes entirely from SceneDB portal components and the
//! engine's own portal-chain composition (`helio-pass-portal-cull` /
//! `helio-pass-portal-instances`). This is the automatic-recursion
//! generalization of `infinite_tunnel`'s single hand-placed corridor: a
//! portal here peers with the actual physical doorway on the opposite wall.
//! Because the engine composes those peer links into chains automatically,
//! each doorway can show the same room repeating away from you for multiple
//! bounces, and a view through one doorway can contain another doorway's
//! projection without any demo-authored copies or continuation anchors.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   Mouse drag  — look around (click to grab cursor)
//!   Tab         — toggle editor mode (on by default): shows a checkerboard
//!                 over each portal opening so you can see where it is.
//!   Escape      — release cursor / exit

mod common;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use common::PortalComponent;
use helio_pass_portal_cull::{
    PortalProjectionBridge, PortalProjectionConfig, SubLevelContents, SubLevelResolver,
    MAX_PORTAL_CHAINS,
};
use pulsar_scenedb::{Entity, SceneDb};
use common::{
    box_mesh, flush_scene_db, make_material, new_scene_db_with_gpu_mirror, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
};

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

/// Room interior half-extent — the room spans [-HALF_SIZE, HALF_SIZE] on
/// every axis, so it's a 2*HALF_SIZE cube.
const HALF_SIZE: f32 = 6.0;
/// Runtime recursive view depth for the demo. The chain row budget below is
/// separate: six mutually visible portals otherwise produce an exponential
/// number of valid paths before GPU culling can discard them.
const PORTAL_RECURSION_DEPTH: usize = 10;
/// Set `CUBE_UNCAPPED_RECURSION=1` to remove the chain-row budget. Recursive
/// expansion is still visibility-driven, so invisible portal branches are
/// rejected before they become SceneDB/GPU rows.
const UNCAPPED_RECURSION_ENV: &str = "CUBE_UNCAPPED_RECURSION";
/// Doorway half-width (local X) / half-height (local Y) in each wall's own
/// face-local frame — also the portal clip opening's half-extent.
const DOOR_HALF_W: f32 = 1.6;
const DOOR_HALF_H: f32 = 2.3;
/// Wall panel half-thickness.
const WALL_T: f32 = 0.15;
/// Far plane — generous enough that a few chain bounces (each one roughly
/// 2*HALF_SIZE farther away) all stay comfortably inside it.
const FAR_PLANE: f32 = 300.0;

fn cube_camera(position: Vec3, yaw: f32, pitch: f32, aspect: f32) -> (Vec3, Camera) {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let forward = Vec3::new(sy * cp, sp, -cy * cp);
    let camera = Camera::perspective_look_at(
        position,
        position + forward,
        Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        aspect,
        0.1,
        FAR_PLANE,
    );
    (forward, camera)
}

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
    last_frame: std::time::Instant,

    cam_pos: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,
    portal_resolver: SubLevelResolver,
    portal_projection_bridge: PortalProjectionBridge,
    portal_projection_entities: Vec<Entity>,
    _light_ids: Vec<Entity>,

    /// Debug-only: when `CUBE_SCREENSHOT` is set, counts frames so a single
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
                        .with_title("Helio – Portal Cube")
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
                application_name: "Portal Cube".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;

        let mut config = RendererConfig::new(size.width, size.height, format);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut renderer = RendererBuilder::new(config, graph_scene_db.clone())
            .with_graph(common::preset())
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        let uncapped_recursion = std::env::var_os(UNCAPPED_RECURSION_ENV).is_some();
        let chain_budget = (!uncapped_recursion).then_some(MAX_PORTAL_CHAINS);
        let projection_capacity = MAX_PORTAL_CHAINS;
        log::info!(
            "[portal_cube] recursion depth={} chain budget={:?} dense row reserve={}",
            PORTAL_RECURSION_DEPTH,
            chain_budget,
            projection_capacity
        );

        // Reserve dense derived rows before authored portal and scene rows.
        // Authored portal entities remain separate from the GPU projection
        // entities populated by the resolver bridge below. In uncapped mode
        // the active rows are rebuilt from camera-visible portal paths each
        // frame; this reserve is only the existing dense SceneDB row arena.
        let projection_entities: Vec<Entity> = (0..projection_capacity)
            .map(|_| scene_db.world.spawn())
            .collect();

        // ── Materials ───────────────────────────────────────────────────────
        let wall_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.75, 0.75, 0.78, 1.0], 0.75, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let frame_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.3, 0.9, 1.0, 1.0], 0.4, 0.0, [0.2, 0.85, 1.0], 2.5),
        );

        // Single shared unit box (half-extent 1 on every axis) — every wall
        // panel and frame piece is this same mesh, scaled/rotated/positioned
        // per instance via its own transform (see `insert_wall_face` below).
        let unit_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
        );

        // ── The room: one wall per axis direction, each with a centered
        // doorway. `up_hint` just needs to not be parallel to `normal` — Y
        // for the four side walls, Z for the floor/ceiling (where normal
        // itself is ±Y). The orthonormalized (right, up, normal) triple this
        // produces is reused directly as the portal's own pose basis below,
        // so the doorway hole and the portal's clip window always agree.
        let faces: [(Vec3, Vec3); 6] = [
            (Vec3::X, Vec3::Y),
            (Vec3::NEG_X, Vec3::Y),
            (Vec3::Y, Vec3::Z),
            (Vec3::NEG_Y, Vec3::Z),
            (Vec3::Z, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y),
        ];

        // There is one authored portal entity per physical opening. The
        // opposite opening is the peer; it is not a hidden target row or a
        // second copy of the same surface. This is the same topology that
        // arbitrary portal scenes use, and lets the resolver produce normal
        // recursive chains without demo-specific anchor records.
        let authored_portal_entities: Vec<Entity> =
            (0..faces.len()).map(|_| scene_db.world.spawn()).collect();
        let opposite_face = [1usize, 0, 3, 2, 5, 4];
        let mut authored_portals = Vec::with_capacity(faces.len());
        for (i, &(normal, up_hint)) in faces.iter().enumerate() {
            let right = up_hint.cross(normal).normalize();
            let up = normal.cross(right).normalize();

            insert_wall_face(
                &mut scene_db.world,
                unit_mesh,
                wall_mat,
                frame_mat,
                normal,
                right,
                up,
            );

            // Pair this opening with the actual opposite physical opening.
            // The authored surface faces into the room: the camera starts on
            // the room side of every wall, and the portal clip convention
            // keeps the half-space in front of this inward-facing normal.
            // Each peer retains its own physical orientation; recursion does
            // not rely on a hidden continuation marker.
            let pose = helio_pass_portal_cull::portal_pose_facing(normal * HALF_SIZE, -normal, up);
            let entity = authored_portal_entities[i];
            let component = PortalComponent::from(helio_pass_portal_cull::PortalRow::new(
                Some(authored_portal_entities[opposite_face[i]]),
                pose.transform,
                [DOOR_HALF_W, DOOR_HALF_H],
            ));
            scene_db.world.insert(entity, component);
            authored_portals.push((entity, component));
        }

        // A central ordinary scene object makes the mapped level contents
        // legible from every doorway. It is deliberately not portal-authored:
        // a portal is a window into the peer level occurrence, so the same
        // ordinary room geometry — including this center reference — is what
        // gets projected through every opening.
        let _ = spawn_object(
            &mut scene_db.world,
            unit_mesh,
            frame_mat,
            Mat4::from_translation(Vec3::ZERO) * Mat4::from_scale(Vec3::splat(0.9)),
            1.6,
        );

        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new([], authored_portals.iter().map(|(e, p)| (*e, p.row()))),
        );
        let projection_bridge = PortalProjectionBridge::with_config(PortalProjectionConfig {
            max_chain_depth: PORTAL_RECURSION_DEPTH,
            max_chains: chain_budget,
            max_coordinate_spaces: None,
        })
        .expect("portal_cube projection depth");
        let initial_position = if std::env::var("CUBE_CLOSEUP").is_ok() {
            Vec3::new(0.0, 0.8, 4.3)
        } else {
            Vec3::new(4.0, 3.0, 4.0)
        };
        let initial_yaw = if std::env::var("CUBE_CLOSEUP").is_ok() {
            std::f32::consts::PI
        } else {
            -0.785
        };
        let initial_pitch = if std::env::var("CUBE_CLOSEUP").is_ok() {
            -0.03
        } else {
            -0.488
        };
        let initial_aspect = size.width as f32 / size.height.max(1) as f32;
        let (_, initial_camera) =
            cube_camera(initial_position, initial_yaw, initial_pitch, initial_aspect);
        let projection_frame = projection_bridge
            .build_visible_from_camera(
                &resolver,
                initial_camera.view_proj_in(glam::DVec3::ZERO),
                initial_position,
            )
            .expect("portal_cube authored portal graph");
        common::publish_portal_projection(
            &mut scene_db.world,
            &projection_frame,
            &projection_entities[..projection_frame.portal_views.len()],
            &projection_entities[..projection_frame.portal_chains.len()],
            projection_entities[0],
        )
        .expect("portal_cube dense projection rows");
        common::set_projection_frame(&mut renderer, &projection_frame);

        // ── A light near the center so every wall reads, plus one per
        // doorway direction so the receding reflections don't go flat black.
        let mut light_ids = Vec::new();
        light_ids.push(spawn_light(
            &mut scene_db.world,
            point_light(
                [0.0, HALF_SIZE * 0.85, 0.0],
                [1.0, 0.98, 0.92],
                4.0,
                HALF_SIZE * 1.8,
            ),
        ));
        // Just 2 more (not one per face — 7 overlapping light-range gizmos
        // in editor mode turned into unreadable clutter) at opposite
        // corners, enough to break up the single center light's flatness.
        for &pos in &[Vec3::new(3.5, 3.0, 3.5), Vec3::new(-3.5, -3.0, -3.5)] {
            light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light([pos.x, pos.y, pos.z], [0.85, 0.92, 1.0], 2.0, HALF_SIZE),
            ));
        }

        // Deferred lighting shades every pixel — including portal
        // duplicates — by real distance to the scene's real lights, and a
        // duplicated reflection can be mapped a full room-length or more
        // away from all of them. Ambient is position-independent, so unlike
        // point lights it reaches reflections just as well as the real
        // room; pushed up well past a typical scene's ambient so the walls
        // read clearly at every reflection depth instead of fading to black
        // a couple of bounces in.
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.6, 0.65, 0.75],
            intensity: 0.35,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        // Editor mode (Tab to toggle) starts on so the portal-opening
        // checkerboard — otherwise invisible, see PortalEditorOverlayPass —
        // is visible by default; press Tab to see the fully seamless game-
        // mode look.
        renderer.set_editor_mode(std::env::var("CUBE_NO_EDITOR").is_err());

        self.state = Some(AppState {
            window,
            surface,
            capture: common::SurfaceCapture::from_env(),
            device,
            queue,
            surface_format: format,
            renderer,
            last_frame: std::time::Instant::now(),
            // Deliberately off the centerline of every doorway (near a
            // corner, not on any face's own axis) — a portal pair here is a
            // pure translation with no zoom, so looking exactly through a
            // doorway's dead center only ever shows the identically-sized
            // doorway hole on the opposite wall, recursively (a nested
            // picture-frame effect) — solid wall content only comes into
            // view off that centerline, same as it would with two real
            // rooms and a real window between them.
            cam_pos: if std::env::var("CUBE_CLOSEUP").is_ok() {
                Vec3::new(0.0, 0.8, 4.3)
            } else {
                Vec3::new(4.0, 3.0, 4.0)
            },
            cam_yaw: if std::env::var("CUBE_CLOSEUP").is_ok() {
                std::f32::consts::PI
            } else {
                -0.785
            },
            cam_pitch: if std::env::var("CUBE_CLOSEUP").is_ok() {
                -0.03
            } else {
                -0.488
            },
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            portal_resolver: resolver,
            portal_projection_bridge: projection_bridge,
            portal_projection_entities: projection_entities,
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
                log::info!("[portal_cube] editor mode: {}", enabled);
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
        const SPEED: f32 = 4.0;
        const SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.4, 1.4);
        self.mouse_delta = (0.0, 0.0);

        let (forward, _) = cube_camera(self.cam_pos, self.cam_yaw, self.cam_pitch, 1.0);
        let (sy, cy) = self.cam_yaw.sin_cos();
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

        let (_, camera) = cube_camera(self.cam_pos, self.cam_yaw, self.cam_pitch, aspect);

        let projection_result = self.portal_projection_bridge.build_visible_from_camera(
            &self.portal_resolver,
            camera.view_proj_in(glam::DVec3::ZERO),
            self.cam_pos,
        );
        match projection_result {
            Ok(projection_frame) => {
                let view_count = projection_frame.portal_views.len();
                let chain_count = projection_frame.portal_chains.len();
                if view_count <= self.portal_projection_entities.len()
                    && chain_count <= self.portal_projection_entities.len()
                {
                    if self.frame_count < 2 {
                        log::info!(
                            "[portal_cube] visible projection frame: views={} chains={} depth={}",
                            view_count,
                            chain_count,
                            PORTAL_RECURSION_DEPTH,
                        );
                    }
                    common::publish_portal_projection(
                        &mut self.scene_db.world,
                        &projection_frame,
                        &self.portal_projection_entities[..view_count],
                        &self.portal_projection_entities[..chain_count],
                        self.portal_projection_entities[0],
                    )
                    .expect("portal_cube visible projection rows");
                    common::set_projection_frame(&mut self.renderer, &projection_frame);
                } else {
                    log::warn!(
                        "[portal_cube] visible projection rows exceed dense row arena: views={} chains={} capacity={}",
                        view_count,
                        chain_count,
                        self.portal_projection_entities.len()
                    );
                }
            }
            Err(error) => {
                log::error!("[portal_cube] visible portal traversal failed: {error}");
            }
        }

        // SceneDB is the authoritative scene. Publish all component rows
        // (including the mesh/material/object rows created at startup) before
        // the render graph reads its GPU mirror.
        flush_scene_db(&self.scene_db, &self.queue);

        if let Err(e) = self
            .capture
            .render_window_frame(&mut self.renderer, &camera, &self.surface)
        {
            log::error!("Render error: {:?}", e);
        }

        // ── Debug screenshot path — see infinite_tunnel.rs for the full
        // rationale (no way to drive a native window interactively here).
        self.frame_count += 1;
        if let Ok(path) = std::env::var("CUBE_SCREENSHOT") {
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

/// Inserts one wall's 4 panels (top/bottom/left/right around a centered
/// doorway) plus an emissive frame collar around the opening, all using the
/// shared unit box mesh scaled per panel via its own transform. `(right,
/// up, normal)` must be orthonormal — see the call site for how that's
/// built from each face's `up_hint`.
fn insert_wall_face(
    world: &mut pulsar_scenedb::World,
    unit_mesh: Entity,
    wall_mat: Entity,
    frame_mat: Entity,
    normal: Vec3,
    right: Vec3,
    up: Vec3,
) {
    let face_center = normal * HALF_SIZE;
    let mut insert_box = |material: Entity,
                          center_right: f32,
                          center_up: f32,
                          center_normal: f32,
                          half_right: f32,
                          half_up: f32,
                          half_normal: f32| {
        let center = face_center + right * center_right + up * center_up + normal * center_normal;
        let transform = Mat4::from_cols(
            (right * half_right).extend(0.0),
            (up * half_up).extend(0.0),
            (normal * half_normal).extend(0.0),
            center.extend(1.0),
        );
        let radius =
            (half_right * half_right + half_up * half_up + half_normal * half_normal).sqrt();
        // Deliberately *not* INSTANCE_FLAG_ALWAYS_VISIBLE — unlike
        // infinite_tunnel's single central corridor segment (world-space,
        // no portal chain ever needs to reconsider it), every wall panel
        // here is real content that portal chains actively frustum-test
        // through their own composed transforms. ALWAYS_VISIBLE would make
        // that test a no-op, so *every* chain thinks *every* panel is in
        // view regardless of where it actually maps to — wildly
        // overselecting and blowing straight through the cull pass's
        // per-group capacity.
        let _ = spawn_object(world, unit_mesh, material, transform, radius);
    };

    // Top / bottom panels span the full width; left / right panels fill the
    // remaining height beside the doorway.
    let vert_half = (HALF_SIZE - DOOR_HALF_H) / 2.0;
    let vert_center = DOOR_HALF_H + vert_half;
    insert_box(
        wall_mat,
        0.0,
        vert_center,
        0.0,
        HALF_SIZE,
        vert_half,
        WALL_T,
    );
    insert_box(
        wall_mat,
        0.0,
        -vert_center,
        0.0,
        HALF_SIZE,
        vert_half,
        WALL_T,
    );
    let horiz_half = (HALF_SIZE - DOOR_HALF_W) / 2.0;
    let horiz_center = DOOR_HALF_W + horiz_half;
    insert_box(
        wall_mat,
        horiz_center,
        0.0,
        0.0,
        horiz_half,
        DOOR_HALF_H,
        WALL_T,
    );
    insert_box(
        wall_mat,
        -horiz_center,
        0.0,
        0.0,
        horiz_half,
        DOOR_HALF_H,
        WALL_T,
    );

    // Emissive frame collar right at the doorway's edge, thin boxes just
    // outside the opening — same purely-decorative role as
    // infinite_tunnel's own portal frames.
    let ft = 0.08;
    insert_box(
        frame_mat,
        0.0,
        DOOR_HALF_H + ft,
        ft,
        DOOR_HALF_W + ft,
        ft,
        ft,
    );
    insert_box(
        frame_mat,
        0.0,
        -(DOOR_HALF_H + ft),
        ft,
        DOOR_HALF_W + ft,
        ft,
        ft,
    );
    insert_box(
        frame_mat,
        DOOR_HALF_W + ft,
        0.0,
        ft,
        ft,
        DOOR_HALF_H + ft,
        ft,
    );
    insert_box(
        frame_mat,
        -(DOOR_HALF_W + ft),
        0.0,
        ft,
        ft,
        DOOR_HALF_H + ft,
        ft,
    );
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
