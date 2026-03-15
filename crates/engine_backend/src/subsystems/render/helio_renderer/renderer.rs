//! HelioRenderer v2 — uses helio-render-v2 (wgpu-native) + WGPUI WgpuSurfaceHandle

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc};
use std::time::Instant;
use glam::{Vec3, Quat, Mat4};

use helio_render_v2::{Renderer as HelioV2, RendererConfig, Camera, SceneLight, GpuMesh};
use helio_render_v2::features::{FeatureRegistry, LightingFeature, BloomFeature, ShadowsFeature, BillboardsFeature};
use gpui::WgpuSurfaceHandle;

use super::core::{CameraInput, RenderMetrics, GpuProfilerData};
use super::gizmo_types::{GizmoStateResource, ViewportMouseInput};

/// All shared state passed to the renderer thread.
#[derive(Clone)]
struct RendererSharedState {
    surface_handle: WgpuSurfaceHandle,
    camera_input: Arc<Mutex<CameraInput>>,
    metrics: Arc<Mutex<RenderMetrics>>,
    gpu_profiler: Arc<Mutex<GpuProfilerData>>,
    gizmo_state: Arc<Mutex<GizmoStateResource>>,
    viewport_mouse_input: Arc<parking_lot::Mutex<ViewportMouseInput>>,
    scene_db: Arc<crate::scene::SceneDb>,
    shutdown: Arc<AtomicBool>,
    game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    physics_query: Option<Arc<crate::services::PhysicsQueryService>>,
}

/// Command sent from UI to renderer thread
pub enum RendererCommand {
    ToggleFeature(String),
}

/// Helio v2 renderer integrated with WGPUI's wgpu surface
pub struct HelioRenderer {
    pub camera_input: Arc<Mutex<CameraInput>>,
    pub metrics: Arc<Mutex<RenderMetrics>>,
    pub gpu_profiler: Arc<Mutex<GpuProfilerData>>,
    pub gizmo_state: Arc<Mutex<GizmoStateResource>>,
    pub viewport_mouse_input: Arc<parking_lot::Mutex<ViewportMouseInput>>,
    pub scene_db: Arc<crate::scene::SceneDb>,
    pub command_sender: mpsc::Sender<RendererCommand>,
    shutdown: Arc<AtomicBool>,
    _render_thread: Option<std::thread::JoinHandle<()>>,
}

impl HelioRenderer {
    pub async fn new(width: u32, height: u32, surface_handle: WgpuSurfaceHandle) -> Self {
        Self::new_with_all(
            width, height, surface_handle,
            None,
            Arc::new(crate::scene::SceneDb::new()),
            None,
        ).await
    }

    pub async fn new_with_all(
        width: u32,
        height: u32,
        surface_handle: WgpuSurfaceHandle,
        game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
        scene_db: Arc<crate::scene::SceneDb>,
        physics_query: Option<Arc<crate::services::PhysicsQueryService>>,
    ) -> Self {
        let camera_input = Arc::new(Mutex::new(CameraInput::new()));
        let metrics = Arc::new(Mutex::new(RenderMetrics::default()));
        let gpu_profiler = Arc::new(Mutex::new(GpuProfilerData::default()));
        let gizmo_state = Arc::new(Mutex::new(GizmoStateResource::default()));
        let viewport_mouse_input = Arc::new(parking_lot::Mutex::new(ViewportMouseInput::default()));
        let (command_sender, command_receiver) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));

        let shared_state = RendererSharedState {
            surface_handle,
            camera_input: camera_input.clone(),
            metrics: metrics.clone(),
            gpu_profiler: gpu_profiler.clone(),
            gizmo_state: gizmo_state.clone(),
            viewport_mouse_input: viewport_mouse_input.clone(),
            scene_db: scene_db.clone(),
            shutdown: shutdown.clone(),
            game_thread_state,
            physics_query,
        };

        let render_thread = std::thread::Builder::new()
            .name("helio-render-v2".to_string())
            .spawn(move || {
                profiling::set_thread_name("Helio Render Thread");
                Self::run_render_thread(width, height, shared_state, command_receiver);
            })
            .map_err(|e| tracing::error!("[HELIO-V2] Failed to spawn render thread: {}", e))
            .ok();

        Self {
            camera_input,
            metrics,
            gpu_profiler,
            gizmo_state,
            viewport_mouse_input,
            scene_db,
            command_sender,
            shutdown,
            _render_thread: render_thread,
        }
    }

    fn run_render_thread(
        width: u32,
        height: u32,
        state: RendererSharedState,
        command_receiver: mpsc::Receiver<RendererCommand>,
    ) {
        let RendererSharedState {
            surface_handle,
            camera_input,
            metrics,
            gpu_profiler,
            gizmo_state: _gizmo_state,
            viewport_mouse_input,
            scene_db,
            shutdown,
            game_thread_state: _,
            physics_query,
        } = state;

        tracing::info!("[HELIO-V2] 🚀 Starting renderer {}x{}", width, height);

        // Get device / queue from the WGPUI surface handle
        let device = Arc::new(surface_handle.device().clone());
        let queue  = Arc::new(surface_handle.queue().clone());
        let format = surface_handle.format();

        tracing::info!("[HELIO-V2] Surface format: {:?}", format);

        // Build the feature registry
        let registry = {
            let mut r = FeatureRegistry::new();
            r.register(Box::new(LightingFeature::new()));
            r.register(Box::new(BloomFeature::new()));
            r.register(Box::new(ShadowsFeature::new()));
            r.register(Box::new(BillboardsFeature::new()));
            r
        };

        let config = RendererConfig::new(width, height, format, registry);

        let mut renderer = match HelioV2::new(device.clone(), queue.clone(), config) {
            Ok(r) => {
                tracing::info!("[HELIO-V2] ✅ Renderer initialised");
                r
            }
            Err(e) => {
                tracing::error!("[HELIO-V2] ❌ Renderer init failed: {}", e);
                return;
            }
        };

        // Add default scene lights
        let sun_dir: Vec3 = Vec3::new(0.4, -0.8, -0.5).normalize();
        let _ = renderer.add_light(SceneLight::directional(
            sun_dir.to_array(),
            [1.0, 0.95, 0.85],
            3.0,
        ));

        // Create a ground plane mesh
        let ground_mesh = renderer.create_mesh_plane([0.0, -0.01, 0.0], 50.0);
        renderer.add_object(&ground_mesh, None, glam::Mat4::IDENTITY);

        // Camera state
        let mut cam_pos   = Vec3::new(0.0, 5.0, 15.0);
        let mut cam_yaw   = 0.0_f32;           // radians, around world-Y
        let mut cam_pitch = -20.0_f32.to_radians(); // radians, looking slightly down

        let start_time   = Instant::now();
        let mut last_frame_time = Instant::now();
        let mut frame_count: u64 = 0;

        tracing::info!("[HELIO-V2] ✅ Entering render loop");

        while !shutdown.load(Ordering::Relaxed) {
            profiling::profile_scope!("HelioV2 Frame");

            // Process commands
            while let Ok(cmd) = command_receiver.try_recv() {
                match cmd {
                    RendererCommand::ToggleFeature(name) => {
                        let _ = renderer.enable_feature(&name);
                        tracing::info!("[HELIO-V2] Toggled feature: {}", name);
                    }
                }
            }

            let now = Instant::now();
            let dt  = (now - last_frame_time).as_secs_f32().min(0.1);
            last_frame_time = now;
            let elapsed = start_time.elapsed().as_secs_f32();

            // ── Camera update ─────────────────────────────────────────────
            {
                if let Ok(mut input) = camera_input.lock() {
                    let speed = input.move_speed * if input.boost { 3.0 } else { 1.0 };
                    let sens  = input.look_sensitivity;

                    // Mouse look
                    if input.mouse_delta_x.abs() > 0.001 || input.mouse_delta_y.abs() > 0.001 {
                        cam_yaw   -= input.mouse_delta_x * sens * 0.01;
                        cam_pitch -= input.mouse_delta_y * sens * 0.01;
                        cam_pitch  = cam_pitch.clamp(
                            -89.0_f32.to_radians(), 89.0_f32.to_radians()
                        );
                        input.mouse_delta_x = 0.0;
                        input.mouse_delta_y = 0.0;
                    }

                    // Basis vectors from yaw/pitch
                    let forward = Vec3::new(
                        cam_pitch.cos() * cam_yaw.sin(),
                        cam_pitch.sin(),
                        -cam_pitch.cos() * cam_yaw.cos(),
                    ).normalize();
                    let right = forward.cross(Vec3::Y).normalize();
                    let up    = right.cross(forward).normalize();

                    // WASD movement
                    cam_pos += forward * input.forward * speed * dt;
                    cam_pos += right   * input.right   * speed * dt;
                    cam_pos += up      * input.up      * speed * dt;

                    // Middle-mouse pan
                    if input.pan_delta_x.abs() > 0.001 || input.pan_delta_y.abs() > 0.001 {
                        cam_pos += right * input.pan_delta_x * 0.01;
                        cam_pos -= up    * input.pan_delta_y * 0.01;
                        input.pan_delta_x = 0.0;
                        input.pan_delta_y = 0.0;
                    }

                    // Scroll zoom
                    if input.zoom_delta.abs() > 0.001 {
                        cam_pos += forward * input.zoom_delta * 0.5;
                        input.zoom_delta = 0.0;
                    }
                }
            }

            // ── Object picking ────────────────────────────────────────────
            {
                let forward = Vec3::new(
                    cam_pitch.cos() * cam_yaw.sin(),
                    cam_pitch.sin(),
                    -cam_pitch.cos() * cam_yaw.cos(),
                ).normalize();
                let right = forward.cross(Vec3::Y).normalize();
                let up    = right.cross(forward).normalize();

                if let Some(mut mouse_input) = viewport_mouse_input.try_lock() {
                    if mouse_input.left_clicked {
                        mouse_input.left_clicked = false;

                        let (sx, sy) = if let Some(bounds) = mouse_input.viewport_bounds {
                            let px = bounds.x + mouse_input.mouse_pos.x * bounds.width;
                            let py = bounds.y + mouse_input.mouse_pos.y * bounds.height;
                            (px / width as f32, py / height as f32)
                        } else {
                            (mouse_input.mouse_pos.x, mouse_input.mouse_pos.y)
                        };

                        let ndc_x = sx * 2.0 - 1.0;
                        let ndc_y = 1.0 - sy * 2.0;
                        let aspect = width as f32 / height as f32;
                        let fov    = 60.0_f32.to_radians();
                        let t      = (fov * 0.5).tan();

                        let ray_dir = (forward
                            + right * (ndc_x * aspect * t)
                            + up    * (ndc_y * t)
                        ).normalize();

                        if let Some(ref pq) = physics_query {
                            if let Some(hit) = pq.raycast(cam_pos, ray_dir, 1000.0) {
                                scene_db.select_object(Some(hit.object_id.clone()));
                            } else {
                                scene_db.select_object(None);
                            }
                        } else {
                            // Fallback sphere-test raycast
                            let mut best: Option<(String, f32)> = None;
                            scene_db.for_each_entry(|e| {
                                if !e.is_visible() { return; }
                                let obj = Vec3::from(e.get_position());
                                let to  = obj - cam_pos;
                                let p   = to.dot(ray_dir);
                                if p > 0.0 {
                                    let cp  = cam_pos + ray_dir * p;
                                    let d   = (obj - cp).length();
                                    let sc  = e.get_scale();
                                    let rad = (sc[0] + sc[1] + sc[2]) / 3.0 * 0.707;
                                    if d < rad {
                                        if best.as_ref().map_or(true, |b| p < b.1) {
                                            best = Some((e.id.clone(), p));
                                        }
                                    }
                                }
                            });
                            scene_db.select_object(best.map(|(id, _)| id));
                        }
                    }
                }
            }

            // ── Physics sync ──────────────────────────────────────────────
            if let Some(ref pq) = physics_query {
                if frame_count % 60 == 0 {
                    pq.sync_from_scene(&scene_db);
                }
            }

            // ── Sync scene objects to renderer ────────────────────────────
            // (helio-render-v2 manages GPU-resident scene via add_object/remove_object
            //  but for now we just re-upload each frame using draw_mesh)

            // ── Build camera ──────────────────────────────────────────────
            let (sw, sh) = surface_handle.size();
            let aspect = if sh > 0 { sw as f32 / sh as f32 } else { 16.0 / 9.0 };
            let forward = Vec3::new(
                cam_pitch.cos() * cam_yaw.sin(),
                cam_pitch.sin(),
                -cam_pitch.cos() * cam_yaw.cos(),
            ).normalize();
            let camera = Camera::perspective(
                cam_pos,
                cam_pos + forward,
                Vec3::Y,
                60.0_f32.to_radians(),
                aspect,
                0.1,
                10000.0,
                elapsed,
            );

            // ── Render ────────────────────────────────────────────────────
            if let Some((view, _size)) = surface_handle.back_view_with_size() {
                if let Err(e) = renderer.render(&camera, &view, dt) {
                    tracing::warn!("[HELIO-V2] render error: {}", e);
                }
                surface_handle.present();
            } else {
                // Surface not ready yet or resizing — back off a little
                std::thread::sleep(std::time::Duration::from_millis(4));
            }

            frame_count += 1;

            // ── Update metrics ────────────────────────────────────────────
            if let Ok(mut m) = metrics.lock() {
                m.fps            = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                m.frame_time_ms  = dt * 1000.0;
                m.frames_rendered = frame_count;
            }
            if let Ok(mut gp) = gpu_profiler.lock() {
                gp.fps            = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                gp.total_frame_ms = dt * 1000.0;
                gp.frame_count    = frame_count;
            }

            if frame_count % 300 == 0 {
                tracing::debug!("[HELIO-V2] Frame {} | {:.1} FPS", frame_count, if dt > 0.0 { 1.0 / dt } else { 0.0 });
            }
        }

        tracing::info!("[HELIO-V2] 🛑 Render thread shutting down");
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    pub fn get_metrics(&self) -> RenderMetrics {
        self.metrics.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn get_gpu_profiler_data(&self) -> GpuProfilerData {
        self.gpu_profiler.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for HelioRenderer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

