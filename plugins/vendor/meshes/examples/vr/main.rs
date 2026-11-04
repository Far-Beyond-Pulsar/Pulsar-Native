//! VR demo: a long showcase hallway (one renderer feature per bay) rendered
//! through the OpenXR render path when a headset is connected, and a plain
//! desktop mirror (forward opaque) with WASD + mouse look when OpenXR
//! initialisation fails.
//!
//! In XR mode, two small emissive cubes are parented to the controller grip
//! poses every frame, so they follow the player's hands exactly.
//!
//! `HelioGraphicsContext` owns optional OpenXR discovery, Vulkan device
//! selection, frame pacing, stereo rendering, and the desktop mirror. The
//! example only reads controller input for its hand markers and locomotion.
//!
//! Controls:
//!   WASD / Space / Shift — fly (desktop mirror mode)
//!   Mouse drag           — look around (click to grab cursor)
//!   Escape               — release cursor / exit
//!   Left stick           — move (XR)
//!   Right stick          — turn (XR)
//!
//! Build / run:
//!   cargo run -p examples --bin vr_demo
//!
//! Without a headset (or without OpenXR + a Vulkan-capable GPU) the demo logs a
//! warning and runs the desktop path; with one, `renderer.render_window_frame()`
//! drives the headset and mirrors its stereo output each frame.
//!
//! The sky (the atmosphere and its clouds) and GI left Helio with their
//! plugins (Pulsar `plugins/vendor/sky`, `plugins/vendor/gi`): this scene
//! has no sky and no GI probes; the background colour and the ambient
//! light stand in.

mod input;
mod scene;
#[path = "../v3_demo_common.rs"]
mod v3_demo_common;

use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use helio::{
    Camera, HelioGraphicsConfig, HelioGraphicsContext, HelioXrInput, RenderMode, Renderer,
    RendererBuilder, RendererConfig,
};
use input::FreeCam;
use pulsar_scenedb::SceneDb;
use v3_demo_common::{new_scene_db_with_gpu_mirror, scene_db_handle, update_object_transform};
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

// ── FPS counter ──────────────────────────────────────────────────────────────

struct FpsCounter {
    frames: u32,
    last_print: Instant,
}

impl FpsCounter {
    fn new() -> Self {
        Self {
            frames: 0,
            last_print: Instant::now(),
        }
    }

    /// Counts frames; logs the rolling FPS once per second.
    fn tick(&mut self) {
        self.frames += 1;
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_print).as_secs_f32();
        if elapsed >= 1.0 {
            log::info!("[fps] {:.0}", self.frames as f32 / elapsed);
            self.frames = 0;
            self.last_print = now;
        }
    }
}

// ── App ──────────────────────────────────────────────────────────────────────

struct App {
    state: Option<AppState>,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

struct AppState {
    window: Arc<Window>,
    graphics: HelioGraphicsContext<'static>,
    device: Arc<wgpu::Device>,
    renderer: Renderer,
    scene_db: SceneDb,
    input: FreeCam,
    /// True when a live OpenXR session is installed in the renderer.
    xr_active: bool,
    #[cfg(not(target_arch = "wasm32"))]
    xr_input: Option<HelioXrInput>,
    /// Player locomotion, applied as the XR stage transform. Position is the stage
    /// origin in world space; yaw is snap/smooth turn about it.
    player_position: Vec3,
    player_yaw: f32,
    animated: scene::Animated,
    start_time: Instant,
    last_frame: Instant,
    fps: FpsCounter,
}

impl AppState {
    /// Read the controllers and walk the player, then publish the result as the XR
    /// stage transform.
    ///
    /// Movement is head-relative on the yaw axis only: pitching your head must not send
    /// you into the floor or the ceiling, which is both disorienting and the fastest way
    /// to make someone sick. Turning is smooth here for simplicity; snap turn is the
    /// gentler default for people prone to motion sickness and is a one-line change.
    #[cfg(not(target_arch = "wasm32"))]
    fn update_locomotion(&mut self, dt: f32) {
        const MOVE_SPEED: f32 = 2.5;
        const TURN_SPEED: f32 = 1.8;
        /// Sticks rest slightly off-centre on most hardware; without a deadzone the
        /// player drifts continuously while standing still.
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

        // Yaw-only basis, so "forward" is where you are facing on the floor plane.
        let (sin, cos) = self.player_yaw.sin_cos();
        let forward = Vec3::new(-sin, 0.0, -cos);
        let right = Vec3::new(cos, 0.0, -sin);
        self.player_position += (forward * move_stick.y + right * move_stick.x) * MOVE_SPEED * dt;

        input.set_locomotion_transform(
            &mut self.renderer,
            glam::Mat4::from_translation(self.player_position)
                * glam::Mat4::from_rotation_y(self.player_yaw),
        );
    }

    /// Reparent the controller cubes to the current grip poses. Located at the
    /// renderer's most recent XR display time, so the cubes stay glued to the
    /// controllers. Hands that are not tracked keep their last transform.
    #[cfg(not(target_arch = "wasm32"))]
    fn update_hands(&mut self) {
        // Offset each cube a little forward and up in grip space so it floats just
        // past the palm instead of being hidden inside the physical controller.
        const HAND_OFFSET: Vec3 = Vec3::new(0.0, 0.03, -0.12);

        let world_from_stage =
            Mat4::from_translation(self.player_position) * Mat4::from_rotation_y(self.player_yaw);
        let Some(input) = &mut self.xr_input else {
            return;
        };
        let Ok(Some(poses)) = input.grip_poses(&self.renderer, &world_from_stage) else {
            return;
        };
        for (i, pose) in poses.into_iter().enumerate() {
            if let (Some(world_pose), Some(cube)) = (pose, self.animated.hand_cubes[i]) {
                let transform = world_pose * Mat4::from_translation(HAND_OFFSET);
                let _ = update_object_transform(
                    &mut self.scene_db.world,
                    &mut self.renderer,
                    cube,
                    transform,
                );
            }
        }
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
                        .with_title("Helio — VR Demo")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("Failed to create window"),
        );

        let size = window.inner_size();
        let mut graphics = pollster::block_on(HelioGraphicsContext::new(
            window.clone(),
            size.width,
            size.height,
            HelioGraphicsConfig {
                vr_support: true,
                application_name: "Helio VR Demo".into(),
                ..Default::default()
            },
        ))
        .expect("graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let config = RendererConfig::new(size.width, size.height, format)
            .with_render_mode(RenderMode::ForwardOpaque);
        let device_arc = Arc::clone(&device);
        device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
            let _ = &device_arc;
            log::error!("[GPU UNCAPTURED ERROR] {e:?}");
        }));

        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let builder = RendererBuilder::new(config.clone(), scene_db_handle(&scene_db))
            .with_pass_build_context(Box::new(|mut ctx| {
                ctx.owns_device = false;
                helio_mesh_host::presets::forward(ctx)
            }));
        let (mut renderer, xr_input) = graphics.build_renderer(builder, config).expect("renderer");
        renderer.set_editor_mode(true);
        let xr_active = graphics.is_xr();

        let animated = scene::build(&mut scene_db.world);

        let state = AppState {
            window,
            graphics,
            device,
            renderer,
            scene_db,
            input: FreeCam::new(),
            xr_active,
            #[cfg(not(target_arch = "wasm32"))]
            xr_input,
            player_position: Vec3::ZERO,
            player_yaw: 0.0,
            animated,
            start_time: Instant::now(),
            last_frame: Instant::now(),
            fps: FpsCounter::new(),
        };
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };

        match event {
            WindowEvent::CloseRequested => {
                log::info!("Shutting down");
                event_loop.exit();
            }

            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                state
                    .graphics
                    .resize(&mut state.renderer, size.width, size.height);
                // In XR mode the graph resolution is fixed by the headset's eye
                // buffer; resizing the mirror window must not rebuild the graph
                // at the window's resolution (that destroys resources cached in
                // pass bind groups and breaks the XR composite).
                if !state.xr_active {
                    state.renderer.set_render_size(size.width, size.height);
                }
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        ..
                    },
                ..
            } => {
                if state.input.cursor_grabbed {
                    state.input.cursor_grabbed = false;
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
                    state.input.keys.insert(key);
                }
                ElementState::Released => {
                    state.input.keys.remove(&key);
                }
            },

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !state.input.cursor_grabbed {
                    let grabbed = state
                        .window
                        .set_cursor_grab(CursorGrabMode::Confined)
                        .or_else(|_| state.window.set_cursor_grab(CursorGrabMode::Locked))
                        .is_ok();
                    if grabbed {
                        state.window.set_cursor_visible(false);
                        state.input.cursor_grabbed = true;
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now.duration_since(state.last_frame).as_secs_f32().min(0.05);
                state.last_frame = now;

                #[cfg(not(target_arch = "wasm32"))]
                if state.xr_active {
                    state.update_locomotion(dt);
                    let scene_time = state.start_time.elapsed().as_secs_f32();
                    scene::animate(
                        &mut state.scene_db.world,
                        &mut state.renderer,
                        &mut state.animated,
                        scene_time,
                    );
                    state.update_hands();
                    // Upload this frame's SceneDB writes to the GPU mirror (Helio#266).
                    v3_demo_common::flush_scene_db(&state.scene_db, state.renderer.queue());
                    {
                        let camera = Camera::perspective_look_at(
                            Vec3::new(0.0, 1.6, 0.0),
                            Vec3::new(0.0, 1.6, -1.0),
                            Vec3::Y,
                            std::f32::consts::FRAC_PI_4,
                            1.0,
                            0.05,
                            200.0,
                        );
                        if let Err(e) = state.graphics.render_frame(&mut state.renderer, &camera) {
                            log::error!("[XR] render error: {e:?}");
                        }
                    }
                    state.input.mouse_delta = (0.0, 0.0);
                    state.fps.tick();
                    state.window.request_redraw();
                    return;
                }

                // Desktop mirror path: WASD + mouse free camera.
                state.input.update(dt);
                let scene_time = state.start_time.elapsed().as_secs_f32();
                scene::animate(
                    &mut state.scene_db.world,
                    &mut state.renderer,
                    &mut state.animated,
                    scene_time,
                );
                let size = state.window.inner_size();
                let aspect = size.width as f32 / size.height.max(1) as f32;
                let camera = state.input.camera(aspect);

                // Upload this frame's SceneDB writes to the GPU mirror (Helio#266).
                v3_demo_common::flush_scene_db(&state.scene_db, state.renderer.queue());
                if let Err(e) = state.graphics.render_frame(&mut state.renderer, &camera) {
                    log::error!("Render error: {e:?}");
                }
                state.fps.tick();
                state.window.request_redraw();
            }

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        let Some(state) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if state.input.cursor_grabbed {
                state.input.mouse_delta.0 += dx as f32;
                state.input.mouse_delta.1 += dy as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

fn main() {
    env_logger::init();
    log::info!("Starting Helio VR demo (OpenXR)");

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("Event loop error");
}
