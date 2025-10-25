//! Main BevyRenderer struct and initialization logic

use bevy::prelude::*;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use super::types::{CameraInput, RenderMetrics, GpuProfilerData, SharedGpuTextures};
use super::resources::*;
use super::components::*;
use super::camera::*;
use super::sync::*;
use super::metrics::*;
use super::scene::*;
use super::textures::*;
use super::gizmos_bevy::*;
use super::viewport_interaction::*;

/// Renderer state
pub struct BevyRenderer {
    pub shared_textures: Arc<Mutex<Option<SharedGpuTextures>>>,
    pub camera_input: Arc<Mutex<CameraInput>>,
    pub metrics: Arc<MetricsResource>,
    pub gpu_profiler: Arc<Mutex<GpuProfilerData>>,
    pub gizmo_state: Arc<Mutex<GizmoStateResource>>,
    pub viewport_mouse_input: Arc<parking_lot::Mutex<ViewportMouseInput>>,
    shutdown: Arc<AtomicBool>,
    _render_thread: Option<std::thread::JoinHandle<()>>,
}

impl BevyRenderer {
    pub async fn new(width: u32, height: u32) -> Self {
        Self::new_with_game_thread(width, height, None).await
    }

    pub async fn new_with_game_thread(
        _width: u32, 
        _height: u32,
        game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    ) -> Self {
        let shared_textures = Arc::new(Mutex::new(None));
        let camera_input = Arc::new(Mutex::new(CameraInput::new()));
        let metrics = Arc::new(MetricsResource::default());
        let gpu_profiler = Arc::new(Mutex::new(GpuProfilerData::default()));
        
        // Create SHARED gizmo state (Arc<Mutex<>> accessible from both GPUI and Bevy)
        let gizmo_state = Arc::new(Mutex::new(GizmoStateResource::default()));
        
        // Create SHARED viewport mouse input (parking_lot::Mutex for better performance)
        let viewport_mouse_input = Arc::new(parking_lot::Mutex::new(ViewportMouseInput::default()));
        
        let shutdown = Arc::new(AtomicBool::new(false));

        let shared_textures_clone = shared_textures.clone();
        let camera_input_clone = camera_input.clone();
        let metrics_clone = metrics.clone();
        let gpu_profiler_clone = gpu_profiler.clone();
        let gizmo_state_clone = gizmo_state.clone();
        let viewport_mouse_input_clone = viewport_mouse_input.clone();
        let shutdown_clone = shutdown.clone();
        let game_thread_clone = game_thread_state.clone();

        let render_thread = std::thread::Builder::new()
            .name("bevy-render".to_string())
            .spawn(move || {
                Self::run_bevy_renderer(
                    RENDER_WIDTH,
                    RENDER_HEIGHT,
                    shared_textures_clone,
                    camera_input_clone,
                    metrics_clone,
                    gpu_profiler_clone,
                    gizmo_state_clone,
                    viewport_mouse_input_clone,
                    shutdown_clone,
                    game_thread_clone,
                );
            })
            .expect("Failed to spawn Bevy render thread");

        // tokio::time::sleep(Duration::from_millis(500)).await;

        Self {
            shared_textures,
            camera_input,
            metrics,
            gpu_profiler,
            gizmo_state,
            viewport_mouse_input,
            shutdown,
            _render_thread: Some(render_thread),
        }
    }

    fn run_bevy_renderer(
        width: u32,
        height: u32,
        shared_textures: Arc<Mutex<Option<SharedGpuTextures>>>,
        camera_input: Arc<Mutex<CameraInput>>,
        metrics: Arc<MetricsResource>,
        gpu_profiler: Arc<Mutex<GpuProfilerData>>,
        gizmo_state: Arc<Mutex<GizmoStateResource>>,
        viewport_mouse_input: Arc<parking_lot::Mutex<ViewportMouseInput>>,
        shutdown: Arc<AtomicBool>,
        game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    ) {
        println!("[BEVY] 🚀 Starting headless renderer {}x{}", width, height);

        let mut app = App::new();

        // Headless mode with DX12 backend (critical for DXGI shared resources!)
        #[cfg(target_os = "windows")]
        let backend_config = bevy::render::settings::Backends::DX12;
        
        #[cfg(target_os = "macos")]
        let backend_config = bevy::render::settings::Backends::METAL;
        
        #[cfg(target_os = "linux")]
        let backend_config = bevy::render::settings::Backends::VULKAN;
        
        app.add_plugins(
            DefaultPlugins
                .set(bevy::window::WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .set(bevy::render::RenderPlugin {
                    render_creation: bevy::render::settings::RenderCreation::Automatic(
                        bevy::render::settings::WgpuSettings {
                            backends: Some(backend_config),
                            ..default()
                        }
                    ),
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>()
        );

        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            Duration::from_secs_f64(1.0 / 3000.0),
        ));

        println!("[BEVY] ✅ Plugins configured");

        // Resources
        app.insert_resource(ClearColor(Color::srgb(0.1, 0.2, 0.3)))
            .insert_resource(camera_input.lock().unwrap().clone())
            .insert_resource(CameraInputResource(camera_input.clone())) // Shared input from input thread
            .insert_resource(SharedTexturesResource(shared_textures.clone()))
            .insert_resource(metrics.as_ref().clone())
            .insert_resource(GpuProfilerResource { data: gpu_profiler.clone() }) // GPU profiler data
            .insert_resource(ShutdownFlag(shutdown.clone()))
            .insert_resource(GameThreadResource(game_thread_state))
            // Shared resources from GPUI (Arc<Mutex<>> accessible from both threads)
            .insert_resource(SharedGizmoStateResource(gizmo_state.clone()))
            .insert_resource(SharedViewportMouseInputResource(viewport_mouse_input.clone()))
            // Local Bevy ECS resources (synced from shared resources each frame)
            .insert_resource(gizmo_state.lock().unwrap().clone()) // Level editor gizmos
            .insert_resource(viewport_mouse_input.lock().clone()) // Viewport mouse interaction
            .insert_resource(GizmoInteractionState::default()) // Gizmo drag state
            .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default()) // Bevy frame time diagnostics
            .add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin::default()); // Bevy GPU render diagnostics
        
        // Main world systems - create textures FIRST, then setup scene
        app.add_systems(Startup, (create_shared_textures_startup, setup_scene).chain())
            .add_systems(Update, check_shutdown)
            // Sync systems - run FIRST to get latest data from main thread
            .add_systems(Update, sync_camera_input_system)         // Sync input thread camera input to Bevy ECS
            .add_systems(Update, sync_gizmo_state_system)          // NEW: Sync GPUI gizmo state to Bevy
            .add_systems(Update, sync_viewport_mouse_input_system) // NEW: Sync GPUI mouse clicks to Bevy
            .add_systems(Update, sync_game_objects_system)         // Sync game thread to Bevy
            // Game systems - run after sync
            .add_systems(Update, camera_movement_system)           // Unreal-style camera controls
            .add_systems(Update, update_gizmo_target_system)       // Keep gizmo centered on selected object
            .add_systems(Update, viewport_click_selection_system)  // Viewport object selection via raycast
            .add_systems(Update, gizmo_drag_system)                // Gizmo dragging for object manipulation
            // Rendering systems - run last
            .add_systems(Update, update_metrics_system)            // Track FPS and frame times
            .add_systems(Update, update_gpu_profiler_system)       // Extract GPU profiler data from Bevy diagnostics
            .add_systems(Update, update_gizmo_visuals)             // Level editor gizmos
            .add_systems(Update, update_selection_highlighting)    // Selection outlines
            .add_systems(Update, debug_rendering_system);          // Add debug system

        // Render world systems
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render_app.insert_resource(SharedTexturesResource(shared_textures.clone()));
            
            #[cfg(target_os = "windows")]
            {
                // Run shared texture creation ONCE on startup in the Render schedule
                // This runs BEFORE extraction, so the GpuImages are ready when camera extracts
                render_app.add_systems(
                    bevy::render::Render,
                    create_shared_textures
                        .run_if(|| {
                            static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                            !ONCE.swap(true, std::sync::atomic::Ordering::Relaxed)
                        })
                        .before(bevy::render::RenderSystems::Render),
                );
            }

            // Extract native handles every frame (for debug/monitoring)
            render_app.add_systems(
                bevy::render::Render,
                extract_native_handles.in_set(bevy::render::RenderSystems::Render)
            );
        }

        println!("[BEVY] ✅ Starting render loop...");
        app.run();
    }

    pub fn update_camera_input(&mut self, input: CameraInput) {
        if let Ok(mut cam) = self.camera_input.lock() {
            *cam = input;
        }
    }

    pub fn get_shared_texture_handles(&self) -> Option<Vec<usize>> {
        // Read from global storage where Bevy stores the handles
        crate::subsystems::render::native_texture::get_shared_handles().map(|handles| {
            handles.iter().map(|h| {
                match h {
                    crate::subsystems::render::NativeTextureHandle::D3D11(ptr) => *ptr,
                }
            }).collect()
        })
    }

    pub fn get_read_index(&self) -> usize {
        // Always use buffer 0 - we're using single-buffered rendering for now
        // TODO: Implement proper double-buffering with camera swapping
        0
    }

    pub fn get_current_native_handle(&self) -> Option<crate::subsystems::render::NativeTextureHandle> {
        let read_idx = self.get_read_index();
        if let Ok(lock) = self.shared_textures.lock() {
            if let Some(ref textures) = *lock {
                if let Ok(handles_lock) = textures.native_handles.lock() {
                    if let Some(ref handles) = *handles_lock {
                        return Some(handles[read_idx].clone());
                    }
                }
            }
        }
        None
    }

    pub fn get_metrics(&self) -> RenderMetrics {
        // Get actual metrics from the tracking resource
        let frames_rendered = self.metrics.frames_rendered.load(Ordering::Relaxed);

        let bevy_fps = self.metrics.fps.lock().ok()
            .map(|f| *f)
            .unwrap_or(0.0);

        let frame_time_ms = self.metrics.frame_time_ms.lock().ok()
            .map(|f| *f)
            .unwrap_or(0.0);

        // Calculate pipeline time from frame time
        let pipeline_time_us = frame_time_ms * 1000.0;

        // Estimate GPU and CPU times (rough estimates based on frame time)
        // GPU typically takes ~60-70% of frame time, CPU ~30-40%
        let gpu_time_us = pipeline_time_us * 0.65;
        let cpu_time_us = pipeline_time_us * 0.35;

        // Calculate dynamic render stats based on scene complexity
        // These are estimates until we can hook into Bevy's actual render stats
        let draw_calls = self.metrics.draw_calls.load(Ordering::Relaxed);
        let vertices_drawn = self.metrics.vertices_drawn.load(Ordering::Relaxed);
        let memory_usage_mb = self.metrics.memory_usage_mb.lock().ok()
            .map(|m| *m)
            .unwrap_or(0.0);

        RenderMetrics {
            fps: bevy_fps,
            frame_time_ms,
            draw_calls,
            memory_usage_mb,
            vertices_drawn,
            frames_rendered,
            bevy_fps,
            pipeline_time_us,
            gpu_time_us,
            cpu_time_us,
        }
    }
    
    /// Get GPU profiler data - detailed render pipeline timing (like Unreal's "stat gpu")
    pub fn get_gpu_profiler_data(&self) -> Option<GpuProfilerData> {
        self.gpu_profiler.lock().ok().map(|data| data.clone())
    }

    pub fn resize(&mut self, _width: u32, _height: u32) {
        // For now, resizing not supported with DXGI shared textures
        // Would require recreating the textures
        println!("[BEVY] ⚠️ Resize not yet implemented for DXGI shared textures");
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

impl Drop for BevyRenderer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn check_shutdown(shutdown: Res<ShutdownFlag>, mut exit: MessageWriter<AppExit>) {
    if shutdown.0.load(Ordering::Acquire) {
        println!("[BEVY] 🛑 Shutdown requested");
        exit.write(AppExit::Success);
    }
}
