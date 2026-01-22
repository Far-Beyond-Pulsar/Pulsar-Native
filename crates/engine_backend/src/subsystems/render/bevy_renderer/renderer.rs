//! Main BevyRenderer struct and initialization logic

use bevy::prelude::*;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use super::core::{
    CameraInput, RenderMetrics, GpuProfilerData, SharedGpuTextures,
    SharedTexturesResource, ShutdownFlag, GameThreadResource, CameraInputResource,
    SharedGizmoStateResource, SharedViewportMouseInputResource, MetricsResource,
    GpuProfilerResource, WgpuProfilerResource, MainCamera, GameObjectId,
};
use super::systems::{
    sync_camera_input_system, camera_movement_system,
    sync_gizmo_state_system, sync_viewport_mouse_input_system,
    sync_game_objects_system, update_gizmo_target_system,
    update_camera_viewport_system,
    update_metrics_system, update_gpu_profiler_system,
    setup_scene, swap_render_buffers_system, debug_rendering_system,
};
use super::textures::*;
use super::gizmos::rendering::{
    GizmoStateResource, update_gizmo_visuals, update_selection_highlighting,
};
use super::interaction::viewport::{
    ViewportMouseInput, GizmoInteractionState, ActiveRaycastTask,
    viewport_click_initiate_raycast_system, viewport_poll_raycast_system, gizmo_drag_system,
    clear_viewport_input_system,
};

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
                profiling::set_thread_name("Bevy Render Thread");
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
        profiling::profile_scope!("BevyRenderer::Run");
        tracing::debug!("[BEVY] 🚀 Starting headless renderer {}x{}", width, height);

        let mut app = App::new();

        // Headless mode with DX12 backend (critical for DXGI shared resources!)
        #[cfg(target_os = "windows")]
        let backend_config = bevy::render::settings::Backends::DX12;
        
        #[cfg(target_os = "macos")]
        let backend_config = bevy::render::settings::Backends::METAL;
        
        #[cfg(target_os = "linux")]
        let backend_config = bevy::render::settings::Backends::VULKAN;
        
        // Configure Bevy's asset server to use the project directory as the asset root
        let project_path = engine_state::get_project_path()
            .unwrap_or("C:\\Users\\redst\\OneDrive\\Documents\\Pulsar_Projects\\blank_project")
            .to_string();
        
        tracing::debug!("[BEVY] 📁 Setting asset path to project directory: {}", project_path);
        println!("[BEVY] 📁 Asset path set to: {}", project_path);
        
        app.add_plugins(
            DefaultPlugins
                .set(bevy::asset::AssetPlugin {
                    file_path: project_path,
                    ..default()
                })
                .set(bevy::window::WindowPlugin {
                    primary_window: Some(bevy::window::Window {
                        present_mode: bevy::window::PresentMode::Immediate,
                        ..default()
                    }),
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
                .disable::<bevy::winit::WinitPlugin>(),
        );
        
        // Add OBJ file support plugin
        app.add_plugins(bevy_obj::ObjPlugin);

        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            Duration::ZERO //Duration::from_secs_f64(1.0 / 3000.0),
        ));

        tracing::debug!("[BEVY] ✅ Plugins configured");

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
            .insert_resource(ActiveRaycastTask::default()) // Async raycast task
            .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default()) // Bevy frame time diagnostics
            .add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin::default()); // Bevy GPU render diagnostics
        
        // Main world systems - create textures FIRST, then setup scene
        app.add_systems(Startup, (create_shared_textures_startup, setup_scene).chain())
            .add_systems(Update, check_shutdown)
            // Sync systems - run FIRST to get latest data from main thread
            .add_systems(Update, sync_camera_input_system)         // Sync input thread camera input to Bevy ECS
            .add_systems(Update, update_camera_viewport_system)    // Update camera viewport to match GPUI bounds
            .add_systems(Update, sync_gizmo_state_system)          // NEW: Sync GPUI gizmo state to Bevy
            .add_systems(Update, sync_viewport_mouse_input_system) // NEW: Sync GPUI mouse clicks to Bevy
            .add_systems(Update, sync_game_objects_system)         // Sync game thread to Bevy
            // Game systems - run after sync
            .add_systems(Update, camera_movement_system)           // Unreal-style camera controls
            .add_systems(Update, update_gizmo_target_system)       // Keep gizmo centered on selected object
            .add_systems(Update, viewport_click_initiate_raycast_system) // FULLY ASYNC: Spawn Bevy worker task
            .add_systems(Update, viewport_poll_raycast_system)     // FULLY ASYNC: Poll result from worker
            .add_systems(Update, gizmo_drag_system)                // Gizmo dragging for object manipulation
            .add_systems(Last, clear_viewport_input_system)        // Clear input flags at end of frame
            // Rendering systems - run last
            .add_systems(Update, update_metrics_system)            // Track FPS and frame times
            .add_systems(Update, update_gpu_profiler_system)       // Extract GPU profiler data from Bevy diagnostics
            .add_systems(Update, update_gizmo_visuals)             // Level editor gizmos
            .add_systems(Update, update_selection_highlighting)    // Selection outlines
            .add_systems(Update, debug_rendering_system)           // Add debug system
            .add_systems(Update, swap_render_buffers_system);      // CRITICAL: Swap buffers for double-buffering

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

        tracing::debug!("[BEVY] ✅ Starting render loop...");
        tracing::debug!("[BEVY] 🎯 Note: Render loop will block this thread until shutdown");
        tracing::debug!("[BEVY] 📊 Shared textures will be created on first render cycle");
        app.run();
        tracing::debug!("[BEVY] 🛑 Render loop ended");
    }

    pub fn update_camera_input(&mut self, input: CameraInput) {
        if let Ok(mut cam) = self.camera_input.lock() {
            *cam = input;
        }
    }

    #[cfg(target_os = "windows")]
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

    #[cfg(not(target_os = "windows"))]
    pub fn get_shared_texture_handles(&self) -> Option<Vec<usize>> {
        // Non-Windows platforms don't support shared texture handles yet
        None
    }

    pub fn get_read_index(&self) -> usize {
        // Read from shared textures' read_index (GPUI reads from this buffer)
        if let Ok(lock) = self.shared_textures.lock() {
            if let Some(ref textures) = *lock {
                return textures.read_index.load(Ordering::Acquire);
            }
        }
        0 // Fallback to buffer 0
    }

    pub fn get_current_native_handle(&self) -> Option<crate::subsystems::render::NativeTextureHandle> {
        let read_idx = self.get_read_index();
        
        // Diagnostic logging (only occasionally to avoid spam)
        static mut CHECK_COUNT: u32 = 0;
        unsafe {
            CHECK_COUNT += 1;
        }
        
        if let Ok(lock) = self.shared_textures.lock() {
            if let Some(ref textures) = *lock {
                if let Ok(handles_lock) = textures.native_handles.lock() {
                    if let Some(ref handles) = *handles_lock {
                        unsafe {
                            //TODO: Re-enable with log levels
                            // if CHECK_COUNT == 1 || CHECK_COUNT % 300 == 0 {
                            //     tracing::debug!("[BEVY] ✅ Returning handle for buffer {}", read_idx);
                            // }
                        }
                        return Some(handles[read_idx].clone());
                    } else {
                        unsafe {
                            if CHECK_COUNT == 1 || CHECK_COUNT % 300 == 0 {
                                tracing::debug!("[BEVY] ⚠️  native_handles is None - textures not created yet");
                            }
                        }
                    }
                } else {
                    unsafe {
                        if CHECK_COUNT == 1 || CHECK_COUNT % 300 == 0 {
                            tracing::debug!("[BEVY] ⚠️  Failed to lock native_handles");
                        }
                    }
                }
            } else {
                unsafe {
                    if CHECK_COUNT == 1 || CHECK_COUNT % 300 == 0 {
                        tracing::debug!("[BEVY] ⚠️  shared_textures is None - SharedGpuTextures not initialized");
                    }
                }
            }
        } else {
            unsafe {
                if CHECK_COUNT == 1 || CHECK_COUNT % 300 == 0 {
                    tracing::debug!("[BEVY] ⚠️  Failed to lock shared_textures");
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
        tracing::debug!("[BEVY] ⚠️ Resize not yet implemented for DXGI shared textures");
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
        tracing::debug!("[BEVY] 🛑 Shutdown requested");
        exit.write(AppExit::Success);
    }
}
