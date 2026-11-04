//! Indoor cathedral example – high complexity
//!
//! A large Gothic cathedral interior: a 60 m nave flanked by two side aisles,
//! 12 stone columns, a raised altar platform with a cross, carved stone pews
//! in 6 rows on each side, three ornate chandeliers, stained-glass window
//! shafts casting coloured light at intervals along both walls, and candle
//! clusters near the altar.
//!
//! No sky atmosphere — the scene relies entirely on the interplay of the
//! chandelier warm-white lights, the cool-coloured stained-glass fills, and
//! a very dim stone-cold ambient to create a moody sacred atmosphere.
//!
//! Controls:
//!   WASD        — move forward/left/back/right
//!   Space/Shift — move up/down
//!   F2          — toggle performance overlay modes
//!   Mouse drag  — look around (click to grab cursor)
//!   Escape      — release cursor / exit

mod common;

use common::{
    box_mesh, make_material, new_scene_db_with_gpu_mirror, plane_mesh, point_light,
    scene_db_handle, spawn_indoor_cathedral_sky, spawn_light, spawn_material, spawn_mesh,
    spawn_object, spawn_object_with_movability, spawn_water_hitbox, spawn_water_volume,
    sphere_mesh, update_light, update_object_transform, update_water_hitbox, WaterHitboxDescriptor,
    WaterVolumeDescriptor,
};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    HelioAction, HelioCommandBridge, Renderer, RendererBuilder, RendererConfig,
};
use helio_bake::BakeConfig;
use helio_pass_perf_overlay::PerfOverlayMode;
use helio_pass_water_sim::WaterSimPass;
use pulsar_scenedb::{Entity, SceneDb};

use std::io::{self, BufRead};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use std::collections::HashSet;

// ── Scene data ────────────────────────────────────────────────────────────────

// Column positions along the nave (Z axis), symmetric at x = ±5.5
const COLUMN_Z: &[f32] = &[-22.0, -14.0, -6.0, 2.0, 10.0, 18.0];

// Stained glass window lights: (x_wall_side, y, z, r, g, b)
// Positive x = right-side windows, negative = left-side; placed just inside the wall
const GLASS_LIGHTS: &[(f32, f32, f32, f32, f32, f32)] = &[
    // Left wall (x ≈ -10.5), windows between columns
    (-10.3, 9.0, -18.0, 0.8, 0.2, 1.0), // violet
    (-10.3, 9.0, -6.0, 0.2, 0.7, 1.0),  // sky blue
    (-10.3, 9.0, 6.0, 0.2, 1.0, 0.4),   // emerald
    (-10.3, 9.0, 18.0, 1.0, 0.7, 0.1),  // gold
    // Right wall (x ≈ +10.5)
    (10.3, 9.0, -18.0, 1.0, 0.2, 0.3), // ruby
    (10.3, 9.0, -6.0, 1.0, 0.5, 0.1),  // amber
    (10.3, 9.0, 6.0, 0.1, 0.8, 0.9),   // teal
    (10.3, 9.0, 18.0, 0.9, 0.1, 0.7),  // magenta
    // Rose window above entrance (back wall, z ≈ +28)
    (0.0, 13.0, 27.0, 1.0, 0.75, 0.3), // warm gold
];

// Chandelier positions (x=0, hanging from y≈19.5, at z intervals)
const CHANDELIER_Z: &[f32] = &[-16.0, 0.0, 16.0];

// Candle cluster positions near the altar (z ≈ -24)
const CANDLES: &[(f32, f32, f32)] = &[
    (-3.0, 1.6, -23.5),
    (-1.5, 1.6, -23.0),
    (0.0, 1.6, -23.5),
    (1.5, 1.6, -23.0),
    (3.0, 1.6, -23.5),
];

// Pew rows: 6 per side, spaced 2.4 m apart starting at z = -20
const PEW_Z_START: f32 = -20.0;
const PEW_Z_STEP: f32 = 3.2;
const PEW_COUNT: usize = 6;

const BALL_RADIUS: f32 = 0.5;
const WATER_SURFACE: f32 = 1.8;
const POOL_HALF_XZ: f32 = 6.0;

fn initial_ball_position() -> glam::Vec3 {
    glam::Vec3::new(0.0, WATER_SURFACE + 4.0, 0.0)
}

fn initial_ball_velocity() -> glam::Vec3 {
    glam::Vec3::new(1.8, 0.0, 1.2)
}

// ─────────────────────────────────────────────────────────────────────────────

/// Build a WaterHitboxDescriptor for a sphere at `new_pos` (previously at `old_pos`).
///
/// Hitboxes are world-space boxes; the simulation maps them into each water
/// volume it overlaps.
fn ball_aabb(
    old_pos: glam::Vec3,
    new_pos: glam::Vec3,
    radius: f32,
    _surface_y: f32,
    _pool_half_xz: f32,
) -> WaterHitboxDescriptor {
    let to_sim = |p: glam::Vec3| -> ([f32; 3], [f32; 3]) {
        (
            (p - glam::Vec3::splat(radius)).to_array(),
            (p + glam::Vec3::splat(radius)).to_array(),
        )
    };
    let (old_min, old_max) = to_sim(old_pos);
    let (new_min, new_max) = to_sim(new_pos);
    WaterHitboxDescriptor {
        old_min,
        old_max,
        new_min,
        new_max,
        edge_softness: 0.15,
        strength: 1.2,
    }
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
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
    renderer: Arc<Mutex<Renderer>>,
    action_rx: Receiver<HelioAction>,
    last_frame: std::time::Instant,

    scene_db: SceneDb,

    // Major structural surfaces
    _floor: Entity,
    _nave_ceiling: Entity,
    _aisle_ceil_l: Entity,
    _aisle_ceil_r: Entity,
    _wall_left_outer: Entity,
    _wall_right_outer: Entity,
    _wall_front: Entity,
    _wall_back: Entity,
    // Colonnade arches (inner walls between nave and aisles, with gaps left for columns)
    _colonnade_l: Vec<Entity>, // wall segments between columns
    _colonnade_r: Vec<Entity>,
    // Columns
    _columns: Vec<Entity>,
    // Altar
    _altar_plinth: Entity,
    _altar_step: Entity,
    _cross_vert: Entity,
    _cross_horiz: Entity,
    // Pews
    _pews_left: Vec<Entity>,
    _pews_right: Vec<Entity>,
    // Chandelier bodies (chain + ring)
    _chandelier_chains: Vec<Entity>,
    _chandelier_rings: Vec<Entity>,

    // Ball physics
    ball_obj_id: Entity,
    ball_hitbox_id: Entity,
    ball_pos: glam::Vec3,
    ball_vel: glam::Vec3,
    ball_prev_pos: glam::Vec3,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    // Debug
    debug_mode: u32,
    perf_overlay_mode: PerfOverlayMode,
    debug_overlay_enabled: bool,

    // Scene state
    chandelier_light_ids: Vec<Entity>,
    candle_light_ids: Vec<Entity>,
    start_time: std::time::Instant,
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
                        .with_title("Helio – Indoor Cathedral")
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
                application_name: "Indoor Cathedral Water".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;

        let config = RendererConfig::new(size.width, size.height, format).with_feature(
            helio_pass_shadow_matrix::ShadowSettings {
                quality: helio::ShadowQuality::Ultra,
                ..Default::default()
            },
        );
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut renderer = RendererBuilder::new(config, graph_scene_db.clone())
            .with_editor_mode(true)
            .with_graph(common::preset())
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        let mat = spawn_material(
            &mut scene_db.world,
            make_material([0.75, 0.72, 0.68, 1.0], 0.85, 0.0, [0.0, 0.0, 0.0], 0.0),
        );

        spawn_indoor_cathedral_sky(&mut scene_db.world);

        // === QUALITY WATER POOL ===
        // Realistic oceanographic parameters for stunning photorealistic water
        let pool = WaterVolumeDescriptor {
            bounds_min: [-6.0, 0.3, -6.0], // 12x12 meter pool, slightly raised
            bounds_max: [6.0, 3.0, 6.0],   // tall enough for 50cm waves above surface
            surface_height: 2.2,           // Water surface higher in the pool for visibility

            // DRAMATIC WAVE PARAMETERS (big rolling swell with visible crests)
            wave_amplitude: 0.5,        // 50cm waves — big, unmistakable motion
            wave_frequency: 0.5,        // Longer period for visible swell propagation
            wave_speed: 6.0,            // Fast, energetic wave motion
            wave_direction: [0.6, 0.3], // Diagonal travel for visual interest
            wave_steepness: 0.6,        // Sharp crests that catch the light and trigger foam

            // WATER OPTICAL PROPERTIES (crystal clear pool water)
            water_color: [0.05, 0.20, 0.30], // Light blue-green for clear water
            extinction: [0.08, 0.04, 0.02], // Very low absorption for crystal clear water (reduced by ~55%)

            // FOAM PARAMETERS (dramatic whitecaps on big swells)
            foam_threshold: 0.4, // Foam appears early — catches every crest
            foam_amount: 0.85,   // Heavy foam coverage for dramatic effect

            // REFLECTION & REFRACTION (physically accurate)
            reflection_strength: 0.65, // Lowered reflectivity for a cleaner pool look
            // Multiplier on the physically-derived displacement (IOR x surface
            // tilt x path length). This was 0.0 to suppress the old constant
            // screen-space offset, which distorted the shallow edges as hard as
            // the deep centre; the current model scales with depth on its own.
            refraction_strength: 1.0,
            fresnel_power: 5.0, // Physically-based fresnel (water IOR ~1.333)

            // CAUSTICS
            // Intensity is scaled up for this scene: at 0.035 m amplitude over a
            // 12 m pool the surface is nearly flat, so it focuses light only
            // weakly and the physical contribution alone would be invisible.
            // The ball's ripples are what actually drive the pattern here.
            caustics_enabled: true,
            caustics_intensity: 3.0,
            caustics_scale: 8.0,
            caustics_speed: 0.0,

            // VOLUMETRIC EFFECTS
            fog_density: 0.0,
            god_rays_intensity: 0.5,
            // Marches the shared hi-Z pyramid; `ssr_steps` is the iteration
            // bound and `ssr_step_size` is unused by the hierarchical traversal.
            ssr_enabled: true,
            ssr_steps: 64,
            ssr_step_size: 0.05,
            ssr_thickness: 0.02,
            // Sim-based rendering parameters (defaults: IOR 1.333, physically-based fresnel)
            ..Default::default()
        };
        spawn_water_volume(&mut scene_db.world, pool);
        // Crank up wind for dramatic wave motion
        if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
            sim.set_wind([0.6, 0.4], 3.5);
            sim.set_wave_scale(0.25);
        }

        // === BOUNCING BALL ===
        // A shiny sphere that bounces perfectly on the water surface, creating ripple waves.
        const BALL_RADIUS: f32 = 0.5;
        const WATER_SURFACE: f32 = 1.8;
        const POOL_HALF_XZ: f32 = 6.0;

        let ball_start = glam::Vec3::new(0.0, WATER_SURFACE + 4.0, 0.0);
        // Give it a diagonal horizontal kick so it travels across the pool
        let ball_start_vel = glam::Vec3::new(1.8, 0.0, 1.2);
        let ball_mesh_id = spawn_mesh(
            &mut scene_db.world,
            sphere_mesh([0.0, 0.0, 0.0], BALL_RADIUS),
        );
        let ball_mat = spawn_material(
            &mut scene_db.world,
            make_material(
                [0.9, 0.15, 0.1, 1.0], // red
                0.2,
                0.0,
                [0.0, 0.0, 0.0],
                0.0,
            ),
        );
        let ball_obj_id = spawn_object_with_movability(
            &mut scene_db.world,
            ball_mesh_id,
            ball_mat,
            glam::Mat4::from_translation(ball_start),
            BALL_RADIUS,
            Some(helio::Movability::Movable),
        )
        .expect("ball object");

        let ball_hitbox_id = spawn_water_hitbox(
            &mut scene_db.world,
            ball_aabb(
                ball_start,
                ball_start,
                BALL_RADIUS,
                WATER_SURFACE,
                POOL_HALF_XZ,
            ),
        );

        // Nave + aisles: total width = 22m (x: -11..+11), length = 60m (z: -28..+28), height = 21m
        // Expand floor to cover full cathedral footprint. 32m radius = 64m square.
        let _floor = spawn_mesh(&mut scene_db.world, plane_mesh([0.0, 0.0, 0.0], 32.0));
        let _wall_back = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [11.0, 10.5, 0.25]),
        );
        let _wall_front = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [11.0, 10.5, 0.25]),
        );
        let _aisle_ceil_l = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [2.5, 0.15, 28.0]),
        );
        let _nave_ceiling = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [6.0, 0.18, 28.0]),
        );
        let _aisle_ceil_r = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [2.5, 0.15, 28.0]),
        );
        let _wall_left_outer = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.25, 7.0, 28.0]),
        );
        let _wall_right_outer = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.25, 7.0, 28.0]),
        );
        let _ = spawn_object(&mut scene_db.world, _floor, mat, glam::Mat4::IDENTITY, 11.0);
        let _ = spawn_object(
            &mut scene_db.world,
            _nave_ceiling,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 21.0, 0.0)),
            28.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _aisle_ceil_l,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-8.5, 11.0, 0.0)),
            28.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _aisle_ceil_r,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(8.5, 11.0, 0.0)),
            28.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _wall_left_outer,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(-11.0, 7.0, 0.0)),
            28.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _wall_right_outer,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(11.0, 7.0, 0.0)),
            28.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _wall_front,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 10.5, 28.0)),
            11.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _wall_back,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 10.5, -28.0)),
            11.0,
        );

        // Colonnade: short wall segments between columns (between column z-positions)
        // 7 segments per side: before first col, between each pair, after last col
        let col_z_all: Vec<f32> = {
            let mut v = vec![-28.0_f32]; // south wall
            v.extend_from_slice(COLUMN_Z);
            v.push(28.0); // north wall
            v
        };
        let _colonnade_l: Vec<Entity> = col_z_all
            .windows(2)
            .map(|w| {
                let mid_z = (w[0] + w[1]) * 0.5;
                let half_len = (w[1] - w[0]) * 0.5 - 0.9; // gap for column
                let id = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [0.25, 5.5, half_len.max(0.1)]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    id,
                    mat,
                    glam::Mat4::from_translation(glam::Vec3::new(-5.5, 5.5, mid_z)),
                    5.5,
                );
                id
            })
            .collect();
        let _colonnade_r: Vec<Entity> = col_z_all
            .windows(2)
            .map(|w| {
                let mid_z = (w[0] + w[1]) * 0.5;
                let half_len = (w[1] - w[0]) * 0.5 - 0.9;
                let id = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [0.25, 5.5, half_len.max(0.1)]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    id,
                    mat,
                    glam::Mat4::from_translation(glam::Vec3::new(5.5, 5.5, mid_z)),
                    5.5,
                );
                id
            })
            .collect();

        // Columns: 0.65 m square, 20 m tall, at x = ±5.5
        let _columns: Vec<Entity> = COLUMN_Z
            .iter()
            .flat_map(|&z| {
                let l = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [0.65, 10.0, 0.65]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    l,
                    mat,
                    glam::Mat4::from_translation(glam::Vec3::new(-5.5, 10.0, z)),
                    10.0,
                );
                let r = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [0.65, 10.0, 0.65]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    r,
                    mat,
                    glam::Mat4::from_translation(glam::Vec3::new(5.5, 10.0, z)),
                    10.0,
                );
                [l, r]
            })
            .collect();

        // Altar: at far end (z = -26)
        let _altar_step = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [5.5, 0.20, 3.0]),
        );
        let _altar_plinth = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [3.0, 0.45, 1.5]),
        );
        let _cross_vert = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [0.18, 2.2, 0.18]),
        );
        let _cross_horiz = spawn_mesh(
            &mut scene_db.world,
            box_mesh([0.0, 0.0, 0.0], [1.0, 0.18, 0.18]),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _altar_step,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.2, -24.5)),
            5.5,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _altar_plinth,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.65, -25.5)),
            3.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _cross_vert,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 3.2, -25.8)),
            2.2,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            _cross_horiz,
            mat,
            glam::Mat4::from_translation(glam::Vec3::new(0.0, 4.5, -25.8)),
            1.0,
        );

        // Pews: long narrow rect3d per row, 6 rows each side
        let _pews_left: Vec<Entity> = (0..PEW_COUNT)
            .map(|i| {
                let z = PEW_Z_START + i as f32 * PEW_Z_STEP;
                let id = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [1.5, 0.45, 0.5]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    id,
                    mat,
                    glam::Mat4::from_translation(glam::Vec3::new(-3.2, 0.45, z)),
                    1.5,
                );
                id
            })
            .collect();
        let _pews_right: Vec<Entity> = (0..PEW_COUNT)
            .map(|i| {
                let z = PEW_Z_START + i as f32 * PEW_Z_STEP;
                let id = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [1.5, 0.45, 0.5]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    id,
                    mat,
                    glam::Mat4::from_translation(glam::Vec3::new(3.2, 0.45, z)),
                    1.5,
                );
                id
            })
            .collect();

        // Chandeliers: vertical chain + horizontal ring at each Z
        let chandelier_mat = spawn_material(
            &mut scene_db.world,
            make_material([0.3, 0.28, 0.25, 1.0], 0.5, 0.8, [0.0, 0.0, 0.0], 0.0),
        );
        let _chandelier_chains: Vec<Entity> = CHANDELIER_Z
            .iter()
            .map(|&z| {
                let id = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [0.06, 2.0, 0.06]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    id,
                    chandelier_mat,
                    glam::Mat4::from_translation(glam::Vec3::new(0.0, 17.5, z)),
                    2.0,
                );
                id
            })
            .collect();
        let _chandelier_rings: Vec<Entity> = CHANDELIER_Z
            .iter()
            .map(|&z| {
                let id = spawn_mesh(
                    &mut scene_db.world,
                    box_mesh([0.0, 0.0, 0.0], [1.2, 0.12, 1.2]),
                );
                let _ = spawn_object(
                    &mut scene_db.world,
                    id,
                    chandelier_mat,
                    glam::Mat4::from_translation(glam::Vec3::new(0.0, 15.2, z)),
                    1.2,
                );
                id
            })
            .collect();

        // Register lights (chandelier & candle light_ids stored for per-frame flicker updates)
        let mut chandelier_light_ids = Vec::new();
        for &z in CHANDELIER_Z {
            chandelier_light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light([0.0_f32, 15.0, z], [1.0, 0.92, 0.78], 8.0, 22.0),
            ));
        }
        // Stained glass shafts. Unlike the removed `Movability::Static` flag,
        // `LightComponent` carries no per-light real-time/baked distinction, so
        // these are spawned as ordinary lights like every other one here.
        for &(x, y, z, r, g, b) in GLASS_LIGHTS {
            spawn_light(
                &mut scene_db.world,
                point_light([x, y, z], [r, g, b], 1.8, 8.0),
            );
        }
        let mut candle_light_ids = Vec::new();
        for &(x, y, z) in CANDLES {
            candle_light_ids.push(spawn_light(
                &mut scene_db.world,
                point_light([x, y, z], [1.0, 0.6, 0.15], 1.2, 4.0),
            ));
        }
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.65, 0.7, 0.85],
            intensity: 0.015,
        });
        renderer.set_clear_color([0.0, 0.0, 0.0, 1.0]);

        // ── Configure baking ──────────────────────────────────────────────
        // Helio automatically extracts all static objects and lights for baking.
        // No need to manually duplicate scene geometry - just specify the config.
        // The renderer never walks the scene: hand it SceneDB's projection
        // first, or there is nothing to bake (Helio#256).
        helio_components::bake(
            &mut renderer,
            helio_mesh_host::bake_scene_from_world(&scene_db.world),
            BakeConfig::fast("indoor_cathedral_water"),
        )
        .expect("bake the cathedral");

        let renderer = Arc::new(Mutex::new(renderer));
        let (bridge, action_rx) = HelioCommandBridge::new();
        let command_bridge = Arc::new(bridge);

        // REPL thread to drive commands from stdin
        {
            let bridge = command_bridge.clone();
            std::thread::spawn(move || {
                let stdin = io::stdin();
                for line in stdin.lock().lines() {
                    match line {
                        Ok(cmd) if !cmd.trim().is_empty() => match bridge.run(&cmd) {
                            Ok(()) => println!("OK: {}", cmd),
                            Err(e) => println!("ERR: {} -> {}", cmd, e),
                        },
                        _ => {}
                    }
                }
            });
        }

        self.state = Some(AppState {
            window,
            surface,
            capture: common::SurfaceCapture::from_env(),
            device,
            queue,
            surface_format: format,
            renderer,
            action_rx,
            last_frame: std::time::Instant::now(),
            scene_db,
            _floor,
            _nave_ceiling,
            _aisle_ceil_l,
            _aisle_ceil_r,
            _wall_left_outer,
            _wall_right_outer,
            _wall_front,
            _wall_back,
            _colonnade_l,
            _colonnade_r,
            _columns,
            _altar_plinth,
            _altar_step,
            _cross_vert,
            _cross_horiz,
            _pews_left,
            _pews_right,
            _chandelier_chains,
            _chandelier_rings,
            ball_obj_id,
            ball_hitbox_id,
            ball_pos: ball_start,
            ball_vel: ball_start_vel,
            ball_prev_pos: ball_start,
            // Start at entrance, looking toward the altar
            cam_pos: glam::Vec3::new(0.0, 2.0, 24.0),
            cam_yaw: 0.0,
            cam_pitch: -0.05,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            debug_mode: 0,
            perf_overlay_mode: PerfOverlayMode::Disabled,
            debug_overlay_enabled: false,
            chandelier_light_ids,
            candle_light_ids,
            start_time: std::time::Instant::now(),
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

            // F1: cycle debug modes (0=normal → 10=shadow heatmap → 11=light-space depth → 0)
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::F1),
                        ..
                    },
                ..
            } => {
                state.debug_mode = match state.debug_mode {
                    0 => 10,
                    10 => 11,
                    _ => 0,
                };
                if let Ok(mut renderer) = state.renderer.lock() {
                    renderer.set_debug_mode(state.debug_mode);
                }
                println!("[debug] shadow debug mode = {}", state.debug_mode);
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::F2),
                        ..
                    },
                ..
            } => {
                state.perf_overlay_mode = match state.perf_overlay_mode {
                    PerfOverlayMode::Disabled => PerfOverlayMode::PassOverdraw,
                    PerfOverlayMode::PassOverdraw => PerfOverlayMode::ShaderComplexity,
                    PerfOverlayMode::ShaderComplexity => PerfOverlayMode::TileLightCount,
                    PerfOverlayMode::TileLightCount => PerfOverlayMode::PassOutput,
                    PerfOverlayMode::PassOutput => PerfOverlayMode::Disabled,
                };
                if let Ok(mut renderer) = state.renderer.lock() {
                    if let Some(pass) =
                        renderer.find_pass_mut::<helio_pass_perf_overlay::PerfOverlayPass>()
                    {
                        pass.set_mode(state.perf_overlay_mode);
                    }
                }
                println!("[debug] perf overlay mode = {:?}", state.perf_overlay_mode);
            }

            // F3: toggle debug overlay
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::F3),
                        ..
                    },
                ..
            } => {
                state.debug_overlay_enabled = !state.debug_overlay_enabled;
                if let Ok(mut renderer) = state.renderer.lock() {
                    if let Some(pass) =
                        renderer.find_pass_mut::<helio_pass_debug_overlay::DebugOverlayPass>()
                    {
                        pass.set_enabled(state.debug_overlay_enabled);
                    }
                }
                println!("[debug] debug overlay = {:?}", state.debug_overlay_enabled);
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
                state.reset_simulation();
                println!("[input] simulation reset");
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
                if let Ok(mut renderer) = state.renderer.lock() {
                    renderer.set_render_size(s.width, s.height);
                }
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
    fn reset_simulation(&mut self) {
        let start_pos = initial_ball_position();
        let start_vel = initial_ball_velocity();

        self.ball_pos = start_pos;
        self.ball_prev_pos = start_pos;
        self.ball_vel = start_vel;
        self.start_time = std::time::Instant::now();

        if let Ok(mut renderer) = self.renderer.lock() {
            let _ = update_object_transform(
                &mut self.scene_db.world,
                &mut renderer,
                self.ball_obj_id,
                glam::Mat4::from_translation(self.ball_pos),
            );
            update_water_hitbox(
                &mut self.scene_db.world,
                self.ball_hitbox_id,
                ball_aabb(
                    self.ball_prev_pos,
                    self.ball_pos,
                    BALL_RADIUS,
                    WATER_SURFACE,
                    POOL_HALF_XZ,
                ),
            );
        }
    }

    fn render(&mut self, dt: f32) {
        const SPEED: f32 = 5.0;
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
        let time = self.start_time.elapsed().as_secs_f32();

        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            200.0,
        );

        // Apply commands from REPL / quark to renderer
        let mut renderer = self.renderer.lock().unwrap();
        while let Ok(action) = self.action_rx.try_recv() {
            match action {
                HelioAction::SetDebugMode(mode) => renderer.set_debug_mode(mode),
                HelioAction::SetEditorMode(enabled) => renderer.set_editor_mode(enabled),
                HelioAction::DebugClear => renderer.debug_clear(),
            }
        }

        // Chandeliers flicker slightly
        let flicker = 1.0 + (time * 9.1).sin() * 0.03 + (time * 5.7).cos() * 0.02;
        // Candle flicker — more pronounced
        let cflicker = 1.0 + (time * 14.3).sin() * 0.07 + (time * 8.9).cos() * 0.05;

        // Update flickering chandelier intensities
        for (i, &id) in self.chandelier_light_ids.iter().enumerate() {
            let z = CHANDELIER_Z[i];
            update_light(
                &mut self.scene_db.world,
                id,
                point_light([0.0_f32, 15.0, z], [1.0, 0.92, 0.78], 8.0 * flicker, 22.0),
            );
        }
        // Update flickering candle intensities
        for (i, &id) in self.candle_light_ids.iter().enumerate() {
            let (x, y, z) = CANDLES[i];
            update_light(
                &mut self.scene_db.world,
                id,
                point_light([x, y, z], [1.0, 0.6, 0.15], 1.2 * cflicker, 4.0),
            );
        }

        // Scene state is persistent — no per-frame setup needed.

        // ---- Bouncing ball physics -------------------------------------------
        const GRAVITY: f32 = -9.8;
        const BALL_RADIUS: f32 = 0.5;
        const WATER_SURFACE: f32 = 1.8;
        const POOL_HALF_XZ: f32 = 6.0;
        const WATER_STIFFNESS: f32 = 26.0;
        const WATER_DAMPING: f32 = 1.2;
        const WATER_DRAG: f32 = 0.35;

        let prev_pos = self.ball_pos;
        self.ball_vel.y += GRAVITY * dt;
        self.ball_pos += self.ball_vel * dt;

        // Water contact: allow the ball to sink and rebound naturally,
        // while avoiding a syrupy, over-damped response.
        let depth = WATER_SURFACE - self.ball_pos.y;
        if depth > 0.0 {
            self.ball_vel.y += WATER_STIFFNESS * depth * dt;
            self.ball_vel.y *= (1.0 - WATER_DAMPING * dt).clamp(0.0, 1.0);
            let drag = (1.0 - WATER_DRAG * dt).clamp(0.0, 1.0);
            self.ball_vel.x *= drag;
            self.ball_vel.z *= drag;
            // Ripple where the ball punches the surface
            if let Some(sim) = renderer.find_pass_mut::<WaterSimPass>() {
                sim.add_drop(self.ball_pos.x, self.ball_pos.z, 0.8, 0.15);
            }
        }

        // Elastic bounce off pool walls (no energy loss)
        let limit = POOL_HALF_XZ - BALL_RADIUS;
        if self.ball_pos.x.abs() > limit {
            self.ball_pos.x = self.ball_pos.x.signum() * limit;
            self.ball_vel.x = -self.ball_vel.x;
        }
        if self.ball_pos.z.abs() > limit {
            self.ball_pos.z = self.ball_pos.z.signum() * limit;
            self.ball_vel.z = -self.ball_vel.z;
        }

        let _ = update_object_transform(
            &mut self.scene_db.world,
            &mut renderer,
            self.ball_obj_id,
            glam::Mat4::from_translation(self.ball_pos),
        );
        update_water_hitbox(
            &mut self.scene_db.world,
            self.ball_hitbox_id,
            ball_aabb(
                prev_pos,
                self.ball_pos,
                BALL_RADIUS,
                WATER_SURFACE,
                POOL_HALF_XZ,
            ),
        );
        self.ball_prev_pos = prev_pos;
        // ---------------------------------------------------------------------

        // Upload this frame's SceneDB writes to the GPU mirror; without it they
        // are never visible to the renderer (Helio#266).
        common::flush_scene_db(&self.scene_db, renderer.queue());
        if let Err(e) = self
            .capture
            .render_window_frame(&mut renderer, &camera, &self.surface)
        {
            log::error!("Render error: {:?}", e);
        }
    }
}
