//! Salva3d Fluid Demo — helio v3
//!
//! An angled faucet emits SPH water particles into a container box.
//! Particles fall, pool on the floor, and collide with boundary particles.
//!
//! Architecture: Direct shared state (no message passing!)
//! - Physics thread: checks shared state directly every loop (2ms sleep for 500Hz rate)
//! - Positions: double-buffered with atomic swap (zero contention!)
//! - Commands: direct mutex-protected state (no channel latency!)
//! - Main thread: writes commands directly to shared state
//!
//! Note: This uses salva3d for fluid simulation with manual boundaries.
//! We avoid rapier integration due to version conflicts (salva3d 0.9 uses rapier 0.18).
//!
//! Controls:
//!   C — toggle physics
//!   R — reset scene
//!   = (Plus) — start tap for 5 seconds
//!   WASD / Space / Shift — fly camera
//!   Escape — release cursor / exit

mod v3_demo_common;
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    box_mesh, despawn_object, make_material, new_scene_db_with_gpu_mirror, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object,
    spawn_object_with_movability, sphere_mesh, update_object_transform,
};

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera, Renderer,
    RendererBuilder, RendererConfig,
};
use salva3d::{
    kernel::CubicSplineKernel,
    object::{Boundary, Fluid},
    solver::{Akinci2013SurfaceTension, DFSPHSolver, XSPHViscosity},
    LiquidWorld,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

struct FluidParticle {
    id: Entity,
}

// Direct shared state (no message passing!)
struct PhysicsSharedState {
    // Double-buffered positions for rendering
    position_buffers: [Mutex<Vec<salva3d::math::Point<f32>>>; 2],
    read_index: AtomicUsize,

    // Timing: track when physics last updated (for latency measurement)
    last_physics_update: Mutex<std::time::Instant>,

    // Command state (checked directly by physics thread)
    pending_spawns: Mutex<
        Vec<(
            Vec<salva3d::math::Point<f32>>,
            Vec<salva3d::math::Vector<f32>>,
            std::time::Instant,
        )>,
    >,
    reset_requested: std::sync::atomic::AtomicBool,
    exit_requested: std::sync::atomic::AtomicBool,
}

impl PhysicsSharedState {
    fn new() -> Self {
        Self {
            position_buffers: [Mutex::new(Vec::new()), Mutex::new(Vec::new())],
            read_index: AtomicUsize::new(0),
            last_physics_update: Mutex::new(std::time::Instant::now()),
            pending_spawns: Mutex::new(Vec::new()),
            reset_requested: std::sync::atomic::AtomicBool::new(false),
            exit_requested: std::sync::atomic::AtomicBool::new(false),
        }
    }

    // Called by physics thread: write positions to write buffer, then atomically swap
    fn write_positions(&self, positions: Vec<salva3d::math::Point<f32>>) {
        let now = std::time::Instant::now();
        let write_idx = 1 - self.read_index.load(Ordering::Acquire);
        *self.position_buffers[write_idx].lock().unwrap() = positions;
        *self.last_physics_update.lock().unwrap() = now;
        self.read_index.store(write_idx, Ordering::Release);
    }

    // Called by render thread: read from read buffer (no contention!)
    fn read_positions(&self) -> Vec<salva3d::math::Point<f32>> {
        let read_idx = self.read_index.load(Ordering::Acquire);
        self.position_buffers[read_idx].lock().unwrap().clone()
    }

    // Get latency: time between physics write and render read
    fn get_physics_latency(&self) -> std::time::Duration {
        let last_update = *self.last_physics_update.lock().unwrap();
        std::time::Instant::now() - last_update
    }

    // Main thread: request spawn (timestamp for latency tracking)
    fn request_spawn(
        &self,
        positions: Vec<salva3d::math::Point<f32>>,
        velocities: Vec<salva3d::math::Vector<f32>>,
    ) {
        self.pending_spawns.lock().unwrap().push((
            positions,
            velocities,
            std::time::Instant::now(),
        ));
    }

    // Physics thread: drain pending spawns
    fn take_pending_spawns(
        &self,
    ) -> Vec<(
        Vec<salva3d::math::Point<f32>>,
        Vec<salva3d::math::Vector<f32>>,
        std::time::Instant,
    )> {
        std::mem::take(&mut *self.pending_spawns.lock().unwrap())
    }
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    last_frame: std::time::Instant,
    frame_count: u64,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    physics_enabled: bool,

    // Physics thread communication (direct shared state - no channels!)
    physics_state: Arc<PhysicsSharedState>,

    particle_radius: f32,
    fluid_density: f32,

    scene_db: SceneDb,
    fluid_particles: Vec<FluidParticle>,
    pending_spawn_count: usize,
    water_material: Entity,
    sphere_mesh: Entity,

    spawn_timer: f32,
    spawn_interval: f32,
    max_particles: usize,

    tap_active: bool,
    tap_timer: f32,
    tap_duration: f32,
}

struct App {
    state: Option<AppState>,
}

// Physics thread function - direct shared state, no message passing
fn physics_thread(shared_state: Arc<PhysicsSharedState>, particle_radius: f32) {
    let smoothing_factor = 2.0;
    let mut liquid_world = LiquidWorld::new(
        DFSPHSolver::<CubicSplineKernel>::new(),
        particle_radius,
        smoothing_factor,
    );

    // Create boundaries
    let mut boundary_positions = Vec::new();
    let container_size = 20.0;
    let wall_height = 6.0;
    let spacing = particle_radius * 2.0;

    // Floor boundary particles
    for x in (-100..=100).map(|i| i as f32 * spacing) {
        for z in (-100..=100).map(|i| i as f32 * spacing) {
            if x.abs() <= container_size && z.abs() <= container_size {
                boundary_positions.push(salva3d::math::Point::new(x, 0.0, z));
            }
        }
    }

    // Wall boundary particles
    for y in (0..=30).map(|i| i as f32 * spacing) {
        if y > wall_height {
            break;
        }
        for z in (-100..=100).map(|i| i as f32 * spacing) {
            if z.abs() <= container_size {
                boundary_positions.push(salva3d::math::Point::new(-container_size, y, z));
                boundary_positions.push(salva3d::math::Point::new(container_size, y, z));
            }
        }
        for x in (-100..=100).map(|i| i as f32 * spacing) {
            if x.abs() <= container_size {
                boundary_positions.push(salva3d::math::Point::new(x, y, -container_size));
                boundary_positions.push(salva3d::math::Point::new(x, y, container_size));
            }
        }
    }

    let boundary = Boundary::new(boundary_positions);
    liquid_world.add_boundary(boundary);

    let gravity = salva3d::math::Vector::new(0.0, -9.81, 0.0);
    let fixed_dt = 1.0 / 120.0; // Physics timestep
    let mut accumulator = 0.0;
    let mut last_time = std::time::Instant::now();

    eprintln!("[Physics] Thread started with double buffering (Unreal-style)");

    let mut loop_count = 0u64;
    let mut total_steps = 0u64;

    loop {
        loop_count += 1;
        let current_time = std::time::Instant::now();
        let frame_time = (current_time - last_time).as_secs_f32();
        last_time = current_time;

        // Clamp frame time to prevent spiral of death
        let frame_time = frame_time.min(0.25);
        accumulator += frame_time;

        // Check for exit first
        if shared_state.exit_requested.load(Ordering::Relaxed) {
            eprintln!("[Physics] Thread exiting");
            return;
        }

        // Check for reset
        if shared_state.reset_requested.swap(false, Ordering::Relaxed) {
            liquid_world = LiquidWorld::new(
                DFSPHSolver::<CubicSplineKernel>::new(),
                particle_radius,
                smoothing_factor,
            );

            // Recreate boundaries
            let mut boundary_positions = Vec::new();
            for x in (-100..=100).map(|i| i as f32 * spacing) {
                for z in (-100..=100).map(|i| i as f32 * spacing) {
                    if x.abs() <= container_size && z.abs() <= container_size {
                        boundary_positions.push(salva3d::math::Point::new(x, 0.0, z));
                    }
                }
            }
            for y in (0..=30).map(|i| i as f32 * spacing) {
                if y > wall_height {
                    break;
                }
                for z in (-100..=100).map(|i| i as f32 * spacing) {
                    if z.abs() <= container_size {
                        boundary_positions.push(salva3d::math::Point::new(-container_size, y, z));
                        boundary_positions.push(salva3d::math::Point::new(container_size, y, z));
                    }
                }
                for x in (-100..=100).map(|i| i as f32 * spacing) {
                    if x.abs() <= container_size {
                        boundary_positions.push(salva3d::math::Point::new(x, y, -container_size));
                        boundary_positions.push(salva3d::math::Point::new(x, y, container_size));
                    }
                }
            }
            let boundary = Boundary::new(boundary_positions);
            liquid_world.add_boundary(boundary);
            accumulator = 0.0;
            eprintln!("[Physics] Reset complete");
        }

        // Process pending spawns (direct state access - measure command latency!)
        let spawns = shared_state.take_pending_spawns();
        if !spawns.is_empty() {
            let mut total_spawned = 0;
            let mut max_latency = std::time::Duration::from_secs(0);

            for (positions, velocities, request_time) in spawns {
                let spawn_latency = current_time - request_time;
                max_latency = max_latency.max(spawn_latency);
                total_spawned += positions.len();

                let mut fluid = Fluid::new(positions, particle_radius, 1000.0);
                fluid
                    .nonpressure_forces
                    .push(Box::new(XSPHViscosity::new(0.5, 0.0)));
                fluid
                    .nonpressure_forces
                    .push(Box::new(Akinci2013SurfaceTension::new(0.5, 0.0)));

                let fluid_handle = liquid_world.add_fluid(fluid);

                // Set velocities
                if let Some(fluid_obj) = liquid_world.fluids_mut().get_mut(fluid_handle) {
                    for (i, vel) in velocities.iter().enumerate() {
                        if i < fluid_obj.velocities.len() {
                            fluid_obj.velocities[i] = *vel;
                        }
                    }
                }
            }

            let total_particles: usize = liquid_world
                .fluids()
                .iter()
                .map(|(_, f)| f.positions.len())
                .sum();
            eprintln!(
                "[Physics] Spawned {} particles | Max latency: {:?} | Total: {}",
                total_spawned, max_latency, total_particles
            );
        }

        // Fixed timestep accumulator pattern (limit to 2 steps max to prevent bursts)
        let mut steps_this_frame = 0;
        let step_start = std::time::Instant::now();
        while accumulator >= fixed_dt && steps_this_frame < 2 {
            liquid_world.step(fixed_dt, &gravity);
            accumulator -= fixed_dt;
            steps_this_frame += 1;
            total_steps += 1;
        }

        // If we're falling behind, just drop the accumulated time
        if accumulator > fixed_dt * 3.0 {
            eprintln!(
                "[Physics] WARNING: Dropping accumulated time ({:.3}s) - physics too slow!",
                accumulator
            );
            accumulator = 0.0;
        }

        let step_duration = step_start.elapsed();

        // Debug: Log physics steps with timing (less frequently)
        if steps_this_frame > 0 && total_steps % 120 == 0 {
            let particle_count = liquid_world
                .fluids()
                .iter()
                .map(|(_, f)| f.positions.len())
                .sum::<usize>();
            eprintln!("[Physics] Stepped {} times in {:?} | Total steps: {} | Particles: {} | Per-step: {:?}",
                steps_this_frame, step_duration, total_steps, particle_count,
                step_duration / steps_this_frame.max(1) as u32);
        }

        // Update shared state (direct write - no channel latency!)
        let write_start = std::time::Instant::now();
        let mut positions = Vec::new();
        for (_, fluid) in liquid_world.fluids().iter() {
            positions.extend(fluid.positions.iter().copied());
        }
        let particle_count = positions.len();

        shared_state.write_positions(positions);
        let write_duration = write_start.elapsed();

        // Log write timing periodically
        if loop_count % 60 == 0 && particle_count > 0 {
            eprintln!(
                "[Physics] Wrote {} positions in {:?} | Loop: {}",
                particle_count, write_duration, loop_count
            );
        }

        // Yield to prevent CPU starvation, but keep loop running fast
        std::thread::yield_now();
    }
}

fn main() {
    env_logger::init();
    log::info!("Starting Salva3d Fluid Demo");
    EventLoop::new()
        .expect("event loop")
        .run_app(&mut App { state: None })
        .expect("run");
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
                        .with_title("Helio — Faucet Fill Box (Salva3d)")
                        .with_inner_size(winit::dpi::LogicalSize::new(1600u32, 900u32)),
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
                application_name: "Rapier Salva".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;
        let fmt = format;

        let config = RendererConfig::new(size.width, size.height, fmt);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut renderer = RendererBuilder::new(config, graph_scene_db.clone())
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.08, 0.08, 0.1],
            intensity: 1.0,
        });

        spawn_light(
            &mut scene_db.world,
            point_light([15.0, 25.0, 15.0], [0.9, 0.9, 0.85], 20.0, 80.0),
        );
        spawn_light(
            &mut scene_db.world,
            point_light([-15.0, 20.0, -10.0], [0.7, 0.8, 1.0], 15.0, 70.0),
        );

        let floor_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.3, 0.3, 0.35, 1.0], 0.8, 0.05, [0.0, 0.0, 0.0], 0.0),
        );
        let wall_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.35, 0.3, 0.28, 1.0], 0.7, 0.02, [0.0, 0.0, 0.0], 0.0),
        );
        let water_material = spawn_material(
            &mut scene_db.world,
            make_material([0.15, 0.4, 0.75, 0.85], 0.1, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let faucet_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.6, 0.6, 0.65, 1.0], 0.3, 0.6, [0.0, 0.0, 0.0], 0.0),
        );

        let sphere_mesh = spawn_mesh(&mut scene_db.world, sphere_mesh([0.0, 0.0, 0.0], 1.0));

        let particle_radius = 0.2;
        let fluid_density = 1000.0;

        // Create shared physics state (direct access - no channels!)
        let physics_state = Arc::new(PhysicsSharedState::new());

        // Spawn physics thread (direct shared state access)
        let pr = particle_radius;
        let state_clone = Arc::clone(&physics_state);
        thread::Builder::new()
            .name("physics".to_string())
            .spawn(move || physics_thread(state_clone, pr))
            .expect("Failed to spawn physics thread");

        eprintln!("[Main] Physics thread started (direct shared state)");

        // Visual representation constants
        let container_size = 20.0;
        let wall_height = 6.0;
        let wall_thickness = 0.5;

        // Render container floor
        let floor_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [20.0, 0.5, 20.0]),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            floor_mesh,
            floor_mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, -0.5, 0.0)),
            20.0,
        );

        // Render container walls
        let wall_thickness = 0.5;

        // Right wall
        let wall_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh(
                [0.0, 0.0, 0.0],
                [wall_thickness * 0.5, wall_height * 0.5, container_size],
            ),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_mesh,
            wall_mat,
            glam::Mat4::from_translation(glam::Vec3::new(
                container_size + wall_thickness * 0.5,
                wall_height * 0.5,
                0.0,
            )),
            10.0,
        );

        // Left wall
        let wall_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh(
                [0.0, 0.0, 0.0],
                [wall_thickness * 0.5, wall_height * 0.5, container_size],
            ),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_mesh,
            wall_mat,
            glam::Mat4::from_translation(glam::Vec3::new(
                -container_size - wall_thickness * 0.5,
                wall_height * 0.5,
                0.0,
            )),
            10.0,
        );

        // Back wall
        let wall_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh(
                [0.0, 0.0, 0.0],
                [container_size, wall_height * 0.5, wall_thickness * 0.5],
            ),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_mesh,
            wall_mat,
            glam::Mat4::from_translation(glam::Vec3::new(
                0.0,
                wall_height * 0.5,
                container_size + wall_thickness * 0.5,
            )),
            10.0,
        );

        // Front wall
        let wall_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh(
                [0.0, 0.0, 0.0],
                [container_size, wall_height * 0.5, wall_thickness * 0.5],
            ),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            wall_mesh,
            wall_mat,
            glam::Mat4::from_translation(glam::Vec3::new(
                0.0,
                wall_height * 0.5,
                -container_size - wall_thickness * 0.5,
            )),
            10.0,
        );

        // Render simple faucet indicator
        let faucet_mesh = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.5, 0.5, 0.5]),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            faucet_mesh,
            faucet_mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 10.0, 0.0)),
            1.0,
        );

        let state = AppState {
            window,
            surface,
            device,
            queue,
            surface_format: fmt,
            renderer,
            last_frame: std::time::Instant::now(),
            frame_count: 0,
            cam_pos: glam::Vec3::new(-15.0, 12.0, 25.0),
            cam_yaw: 0.5,
            cam_pitch: -0.3,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            physics_enabled: true,
            physics_state,
            particle_radius,
            fluid_density,
            scene_db,
            fluid_particles: Vec::new(),
            pending_spawn_count: 0,
            water_material,
            sphere_mesh,
            spawn_timer: 0.0,
            spawn_interval: 0.06,
            max_particles: 1000,
            tap_active: false,
            tap_timer: 0.0,
            tap_duration: 5.0,
        };

        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
                        physical_key: PhysicalKey::Code(KeyCode::KeyC),
                        ..
                    },
                ..
            } => {
                state.physics_enabled = !state.physics_enabled;
                eprintln!("physics enabled={}", state.physics_enabled);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::KeyR),
                        ..
                    },
                ..
            } => {
                state.reset_scene();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Equal),
                        ..
                    },
                ..
            } => {
                state.tap_active = true;
                state.tap_timer = 0.0;
                state.spawn_timer = 0.0; // Reset spawn timer so particles start immediately
                eprintln!(
                    "Tap activated for {} seconds (particles: {})",
                    state.tap_duration,
                    state.fluid_particles.len()
                );
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
                        desired_maximum_frame_latency: 1,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.renderer.set_render_size(s.width, s.height);
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let dt = (now - state.last_frame).as_secs_f32().min(0.033);
                state.last_frame = now;
                state.render(dt);
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
        if let Some(s) = &mut self.state {
            s.window.request_redraw();
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        // Tell physics thread to exit (direct state access)
        self.physics_state
            .exit_requested
            .store(true, Ordering::Relaxed);
    }
}

impl AppState {
    fn reset_scene(&mut self) {
        eprintln!("Resetting scene...");

        // Clear all fluid particles
        for particle in self.fluid_particles.drain(..) {
            let _ = despawn_object(&mut self.scene_db.world, &mut self.renderer, particle.id);
        }

        // Request reset (direct state access)
        self.physics_state
            .reset_requested
            .store(true, Ordering::Relaxed);

        self.pending_spawn_count = 0;
        self.spawn_timer = 0.0;
    }

    fn spawn_fluid_parcel(&mut self) {
        if self.fluid_particles.len() + self.pending_spawn_count >= self.max_particles {
            if self.tap_active {
                eprintln!(
                    "Max particles ({}) reached! Press R to reset.",
                    self.max_particles
                );
                self.tap_active = false; // Stop trying to spawn
            }
            return;
        }

        // Spawn position at the faucet (center, above container)
        let spawn_pos = glam::Vec3::new(0.0, 9.0, 0.0);

        // Create a tight vertical stream (2x2 horizontal, 1 vertical)
        let horizontal_size = 2;
        let spacing = self.particle_radius * 1.8;

        let mut positions = Vec::new();
        let mut velocities = Vec::new();

        // Downward initial velocity for stream effect
        let initial_velocity = salva3d::math::Vector::new(0.0, -5.0, 0.0);

        for x in 0..horizontal_size {
            for z in 0..horizontal_size {
                if self.fluid_particles.len() + self.pending_spawn_count + positions.len()
                    >= self.max_particles
                {
                    break;
                }

                let offset = glam::Vec3::new(
                    (x as f32 - horizontal_size as f32 * 0.5 + 0.5) * spacing,
                    0.0,
                    (z as f32 - horizontal_size as f32 * 0.5 + 0.5) * spacing,
                );
                let pos = spawn_pos + offset;
                positions.push(salva3d::math::Point::new(pos.x, pos.y, pos.z));
                velocities.push(initial_velocity);
            }
        }

        if positions.is_empty() {
            return;
        }

        let particle_count = positions.len();

        eprintln!(
            "[Main] Spawning {} particles at tap position | Total visual: {} | Pending: {}",
            particle_count,
            self.fluid_particles.len(),
            self.pending_spawn_count
        );

        // Request spawn (direct state access - no channel latency!)
        self.physics_state.request_spawn(positions, velocities);

        // Track pending particles (will be created when we receive position updates)
        self.pending_spawn_count += particle_count;
    }

    fn update_from_physics(&mut self) {
        let read_start = std::time::Instant::now();

        // Measure latency: time since physics last wrote data
        let physics_latency = self.physics_state.get_physics_latency();

        // Read positions from read buffer (single atomic snapshot - no race condition!)
        let positions = self.physics_state.read_positions();
        let physics_particle_count = positions.len();

        let read_duration = read_start.elapsed();

        // Log every 60 frames
        if self.frame_count % 60 == 0 {
            eprintln!(
                "[Render] Latency: {:?} | Read {} positions in {:?} | Visual: {} | First pos: {:?}",
                physics_latency,
                physics_particle_count,
                read_duration,
                self.fluid_particles.len(),
                positions.first().map(|p| (p.x, p.y, p.z))
            );
        }

        // Create visual particles for new physics particles
        let initial_visual_count = self.fluid_particles.len();
        while self.fluid_particles.len() < physics_particle_count {
            let transform = glam::Mat4::from_scale(glam::Vec3::splat(self.particle_radius));
            let obj = spawn_object_with_movability(
                &mut self.scene_db.world,
                self.sphere_mesh,
                self.water_material,
                transform,
                self.particle_radius,
                Some(helio::Movability::Movable),
            )
            .expect("insert particle");
            self.fluid_particles.push(FluidParticle { id: obj });

            if self.pending_spawn_count > 0 {
                self.pending_spawn_count -= 1;
            }
        }

        if self.fluid_particles.len() != initial_visual_count {
            eprintln!(
                "[Render] Created {} new visual particles (total: {})",
                self.fluid_particles.len() - initial_visual_count,
                self.fluid_particles.len()
            );
        }

        // Update transforms using the same positions snapshot (consistent data!)
        let update_start = std::time::Instant::now();
        for (i, pos) in positions.iter().enumerate() {
            if i >= self.fluid_particles.len() {
                eprintln!(
                    "[Render] WARNING: Position index {} >= visual particles {}",
                    i,
                    self.fluid_particles.len()
                );
                break;
            }

            let transform = glam::Mat4::from_translation(glam::Vec3::new(pos.x, pos.y, pos.z))
                * glam::Mat4::from_scale(glam::Vec3::splat(self.particle_radius));
            let _ = update_object_transform(
                &mut self.scene_db.world,
                &mut self.renderer,
                self.fluid_particles[i].id,
                transform,
            );
        }

        if self.frame_count % 60 == 0 {
            eprintln!(
                "[Render] Updated {} transforms in {:?}",
                positions.len().min(self.fluid_particles.len()),
                update_start.elapsed()
            );
        }
    }

    fn render(&mut self, dt: f32) {
        const SPEED: f32 = 15.0;
        const SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.5, 1.5);
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
            self.cam_pos.y += SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos.y -= SPEED * dt;
        }

        if self.physics_enabled {
            // Update tap timer
            if self.tap_active {
                self.tap_timer += dt;
                if self.tap_timer >= self.tap_duration {
                    self.tap_active = false;
                    self.tap_timer = 0.0;
                    eprintln!(
                        "Tap deactivated (total particles: {})",
                        self.fluid_particles.len()
                    );
                }
            }

            // Spawn fluid parcels periodically when tap is active
            if self.tap_active {
                self.spawn_timer += dt;
                if self.spawn_timer >= self.spawn_interval {
                    self.spawn_timer = 0.0;
                    self.spawn_fluid_parcel();

                    // Show progress every 1 second
                    if (self.tap_timer * 2.0) as i32 != ((self.tap_timer - dt) * 2.0) as i32 {
                        let remaining = self.tap_duration - self.tap_timer;
                        if remaining > 0.0 {
                            eprintln!(
                                "  Tap running: {:.1}s remaining, {} particles",
                                remaining,
                                self.fluid_particles.len()
                            );
                        }
                    }
                }
            }
        }

        // Update visual particles from physics thread (runs independently)
        self.update_from_physics();

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_3,
            aspect,
            0.1,
            300.0,
        );

        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render error: {:?}", e);
        }
        self.frame_count += 1;

        if self.frame_count % 120 == 0 {
            let fps = 120.0
                / (std::time::Instant::now() - self.last_frame
                    + std::time::Duration::from_secs_f32(dt * 119.0))
                .as_secs_f32();
            eprintln!(
                "[Render] Frame {} | Particles: {} | FPS: {:.1} | Physics: {}",
                self.frame_count,
                self.fluid_particles.len(),
                fps,
                self.physics_enabled
            );
        }
    }
}
