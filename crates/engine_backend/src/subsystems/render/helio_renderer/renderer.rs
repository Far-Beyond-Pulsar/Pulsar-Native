//! Main HelioRenderer struct and initialization logic
//! Matches BevyRenderer's API but uses blade-graphics + Helio features

use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};
use glam::{Vec3, Mat4};

use helio_core::{create_cube_mesh, create_sphere_mesh, MeshBuffer, TextureManager};
use helio_render::{FpsCamera, FeatureRenderer, TransformUniforms};
use helio_features::FeatureRegistry;
use helio_feature_base_geometry::BaseGeometry;
use helio_feature_lighting::BasicLighting;
use helio_feature_materials::BasicMaterials;
use helio_feature_procedural_shadows::ProceduralShadows;
use helio_feature_bloom::Bloom;
use helio_feature_billboards::BillboardFeature;

use super::core::{CameraInput, RenderMetrics, GpuProfilerData, SharedGpuTextures};

// Import gizmo types from bevy_renderer (we'll reuse these for now)
pub use crate::subsystems::render::bevy_renderer::gizmos::rendering::{
    GizmoType as BevyGizmoType, GizmoAxis as BevyGizmoAxis, GizmoStateResource,
};
pub use crate::subsystems::render::bevy_renderer::interaction::viewport::{
    ViewportMouseInput, GizmoInteractionState, ActiveRaycastTask, RaycastResult,
};

/// Helio-based renderer matching BevyRenderer's API
pub struct HelioRenderer {
    pub shared_textures: Arc<Mutex<Option<SharedGpuTextures>>>,
    pub camera_input: Arc<Mutex<CameraInput>>,
    pub metrics: Arc<Mutex<RenderMetrics>>,
    pub gpu_profiler: Arc<Mutex<GpuProfilerData>>,
    pub gizmo_state: Arc<Mutex<GizmoStateResource>>,
    pub viewport_mouse_input: Arc<parking_lot::Mutex<ViewportMouseInput>>,
    shutdown: Arc<AtomicBool>,
    _render_thread: Option<std::thread::JoinHandle<()>>,
}

impl HelioRenderer {
    pub async fn new(width: u32, height: u32) -> Self {
        Self::new_with_game_thread(width, height, None).await
    }

    pub async fn new_with_game_thread(
        width: u32,
        height: u32,
        game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    ) -> Self {
        let shared_textures = Arc::new(Mutex::new(None));
        let camera_input = Arc::new(Mutex::new(CameraInput::new()));
        let metrics = Arc::new(Mutex::new(RenderMetrics::default()));
        let gpu_profiler = Arc::new(Mutex::new(GpuProfilerData::default()));
        let gizmo_state = Arc::new(Mutex::new(GizmoStateResource::default()));
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
            .name("helio-render".to_string())
            .spawn(move || {
                profiling::set_thread_name("Helio Render Thread");
                Self::run_helio_renderer(
                    width,
                    height,
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
            .expect("Failed to spawn Helio render thread");

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

    fn run_helio_renderer(
        width: u32,
        height: u32,
        _shared_textures: Arc<Mutex<Option<SharedGpuTextures>>>,
        camera_input: Arc<Mutex<CameraInput>>,
        metrics: Arc<Mutex<RenderMetrics>>,
        gpu_profiler: Arc<Mutex<GpuProfilerData>>,
        _gizmo_state: Arc<Mutex<GizmoStateResource>>,
        _viewport_mouse_input: Arc<parking_lot::Mutex<ViewportMouseInput>>,
        shutdown: Arc<AtomicBool>,
        _game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    ) {
        profiling::profile_scope!("HelioRenderer::Run");
        tracing::info!("[HELIO] 🚀 Starting headless renderer {}x{}", width, height);

        // Initialize blade-graphics context (headless)
        let context = Arc::new(unsafe {
            blade_graphics::Context::init(blade_graphics::ContextDesc {
                presentation: false, // Headless - we'll handle presentation via DXGI
                validation: cfg!(debug_assertions),
                timing: false,
                capture: false,
                overlay: false,
                device_id: 0,
            })
            .expect("Failed to initialize blade-graphics context")
        });

        // Create test meshes
        let cube_mesh = MeshBuffer::from_mesh(&context, "cube", &create_cube_mesh(1.0));
        let sphere_mesh = MeshBuffer::from_mesh(&context, "sphere", &create_sphere_mesh(0.5, 32, 32));

        // Create gizmo meshes
        let arrow_mesh = MeshBuffer::from_mesh(
            &context,
            "gizmo_arrow",
            &super::gizmos::create_arrow_mesh(1.0, 0.05, 0.1, 0.3),
        );
        let torus_mesh = MeshBuffer::from_mesh(
            &context,
            "gizmo_torus",
            &super::gizmos::create_torus_mesh(1.0, 0.05, 32, 16),
        );

        // Setup texture manager (optional for now)
        let texture_manager = Arc::new(TextureManager::new(context.clone()));

        // Setup rendering features
        let mut base_geometry = BaseGeometry::new();
        base_geometry.set_texture_manager(texture_manager.clone());
        let base_shader = base_geometry.shader_template().to_string();

        let mut shadows = ProceduralShadows::new().with_ambient(0.2);
        shadows.set_texture_manager(texture_manager.clone());

        let mut billboards = BillboardFeature::new();
        billboards.set_texture_manager(texture_manager.clone());

        let registry = FeatureRegistry::builder()
            .with_feature(base_geometry)
            .with_feature(BasicLighting::new())
            .with_feature(BasicMaterials::new())
            .with_feature(shadows)
            .with_feature(Bloom::new())
            .with_feature(billboards)
            .build();

        // Create offscreen render target (we'll replace this with DXGI shared texture later)
        let render_target = context.create_texture(blade_graphics::TextureDesc {
            name: "helio_render_target",
            format: blade_graphics::TextureFormat::Rgba8Unorm,
            size: blade_graphics::Extent {
                width,
                height,
                depth: 1,
            },
            dimension: blade_graphics::TextureDimension::D2,
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            usage: blade_graphics::TextureUsage::TARGET | blade_graphics::TextureUsage::RESOURCE,
            external: None,
        });

        let mut renderer = FeatureRenderer::new(
            context.clone(),
            blade_graphics::TextureFormat::Rgba8Unorm,
            width,
            height,
            registry,
            &base_shader,
        )
        .expect("Failed to create FeatureRenderer");

        let mut command_encoder = context.create_command_encoder(blade_graphics::CommandEncoderDesc {
            name: "helio_main",
            buffer_count: 2,
        });

        // Camera setup
        let mut camera = FpsCamera::new(Vec3::new(0.0, 5.0, 15.0));
        camera.pitch = -20.0_f32.to_radians();

        let start_time = Instant::now();
        let mut last_frame_time = Instant::now();
        let mut frame_count: u64 = 0;

        tracing::info!("[HELIO] ✅ Renderer initialized, entering render loop");

        // Main render loop
        while !shutdown.load(Ordering::Relaxed) {
            profiling::profile_scope!("Helio Frame");
            
            let now = Instant::now();
            let delta_time = (now - last_frame_time).as_secs_f32();
            last_frame_time = now;

            // Update camera from input
            if let Ok(input) = camera_input.lock() {
                camera.update_movement(input.forward, input.right, input.up, delta_time);
                // TODO: Apply mouse delta for rotation
            }

            // Start frame
            command_encoder.start();
            command_encoder.init_texture(render_target);

            let aspect = width as f32 / height as f32;
            let camera_uniforms = camera.build_camera_uniforms(60.0, aspect);

            // Build scene meshes from GameState (if available)
            let mut meshes = Vec::new();
            
            // Load objects from GameState if available
            if let Some(ref game_state_arc) = _game_thread_state {
                if let Ok(game_state) = game_state_arc.lock() {
                    for obj in &game_state.objects {
                        if !obj.active {
                            continue;
                        }
                        
                        // Convert GameObject to transform matrix
                        let translation = Mat4::from_translation(Vec3::new(
                            obj.position[0],
                            obj.position[1],
                            obj.position[2],
                        ));
                        
                        let rotation = Mat4::from_rotation_y(obj.rotation[1])
                            * Mat4::from_rotation_x(obj.rotation[0])
                            * Mat4::from_rotation_z(obj.rotation[2]);
                        
                        let scale = Mat4::from_scale(Vec3::new(
                            obj.scale[0],
                            obj.scale[1],
                            obj.scale[2],
                        ));
                        
                        let transform = translation * rotation * scale;
                        
                        // Use cube mesh for now (TODO: load actual meshes)
                        meshes.push((TransformUniforms::from_matrix(transform), &cube_mesh));
                    }
                }
            }
            
            // Add ground plane
            let ground_transform = Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0))
                * Mat4::from_scale(Vec3::new(10.0, 0.1, 10.0));
            meshes.push((TransformUniforms::from_matrix(ground_transform), &cube_mesh));

            // If no objects in scene, add demo objects
            if meshes.len() <= 1 {
                let elapsed = (now - start_time).as_secs_f32();
                
                // Rotating sphere
                let sphere_transform = Mat4::from_translation(Vec3::new(
                    (elapsed * 0.5).sin() * 3.0,
                    2.0,
                    (elapsed * 0.5).cos() * 3.0,
                )) * Mat4::from_rotation_y(elapsed);
                meshes.push((TransformUniforms::from_matrix(sphere_transform), &sphere_mesh));
                
                // Static cube
                let cube_transform = Mat4::from_translation(Vec3::new(-3.0, 1.5, 0.0));
                meshes.push((TransformUniforms::from_matrix(cube_transform), &cube_mesh));
            }

            // Render gizmos if object is selected
            if let Ok(gizmo_state_lock) = _gizmo_state.lock() {
                if gizmo_state_lock.selected_object_id.is_some() {
                    // Get gizmo target position (selected object position)
                    let gizmo_position = Vec3::new(
                        gizmo_state_lock.target_position[0],
                        gizmo_state_lock.target_position[1],
                        gizmo_state_lock.target_position[2],
                    );
                    
                    let gizmo_scale = 0.5; // Scale down gizmo size
                    
                    use super::gizmos::{GizmoType, GizmoAxis, create_gizmo_arrow_transform, create_gizmo_torus_transform};
                    use crate::subsystems::render::bevy_renderer::BevyGizmoType;
                    
                    match gizmo_state_lock.gizmo_type {
                        BevyGizmoType::Translate => {
                            // Render 3 arrows for X, Y, Z
                            for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                                let transform = create_gizmo_arrow_transform(gizmo_position, axis, gizmo_scale);
                                meshes.push((TransformUniforms::from_matrix(transform), &arrow_mesh));
                            }
                        }
                        BevyGizmoType::Rotate => {
                            // Render 3 toruses for X, Y, Z
                            for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                                let transform = create_gizmo_torus_transform(gizmo_position, axis, gizmo_scale);
                                meshes.push((TransformUniforms::from_matrix(transform), &torus_mesh));
                            }
                        }
                        BevyGizmoType::Scale => {
                            // Render 3 cubes for X, Y, Z scale handles
                            for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                                let offset = match axis {
                                    GizmoAxis::X => Vec3::new(1.0, 0.0, 0.0),
                                    GizmoAxis::Y => Vec3::new(0.0, 1.0, 0.0),
                                    GizmoAxis::Z => Vec3::new(0.0, 0.0, 1.0),
                                    _ => Vec3::ZERO,
                                } * gizmo_scale;
                                
                                let transform = Mat4::from_translation(gizmo_position + offset)
                                    * Mat4::from_scale(Vec3::splat(0.2 * gizmo_scale));
                                meshes.push((TransformUniforms::from_matrix(transform), &cube_mesh));
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Render scene
            let render_target_view = context.create_texture_view(
                render_target,
                blade_graphics::TextureViewDesc {
                    name: "helio_render_view",
                    format: blade_graphics::TextureFormat::Rgba8Unorm,
                    dimension: blade_graphics::ViewDimension::D2,
                    subresources: &blade_graphics::TextureSubresources::default(),
                },
            );
            renderer.render(
                &mut command_encoder,
                render_target_view,
                camera_uniforms,
                &meshes,
                delta_time,
            );

            // Submit and wait (for now - in real implementation we'd handle DXGI sync differently)
            let sync_point = context.submit(&mut command_encoder);
            context.wait_for(&sync_point, !0);

            frame_count += 1;

            // Update metrics
            if let Ok(mut m) = metrics.lock() {
                m.fps = if delta_time > 0.0 { 1.0 / delta_time } else { 0.0 };
                m.frame_time_ms = delta_time * 1000.0;
                m.frames_rendered = frame_count;
            }

            if let Ok(mut gp) = gpu_profiler.lock() {
                gp.fps = if delta_time > 0.0 { 1.0 / delta_time } else { 0.0 };
                gp.total_frame_ms = delta_time * 1000.0;
                gp.frame_count = frame_count;
            }

            // Target ~60 FPS for now
            std::thread::sleep(Duration::from_millis(16));
        }

        tracing::info!("[HELIO] 🛑 Render thread shutting down");
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    pub fn get_metrics(&self) -> RenderMetrics {
        self.metrics.lock().unwrap().clone()
    }

    pub fn get_gpu_profiler_data(&self) -> GpuProfilerData {
        self.gpu_profiler.lock().unwrap().clone()
    }

    pub fn get_read_index(&self) -> usize {
        // Read from shared textures' read_index (GPUI reads from this buffer)
        if let Ok(lock) = self.shared_textures.lock() {
            if let Some(ref textures) = *lock {
                return textures.read_index.load(std::sync::atomic::Ordering::Acquire);
            }
        }
        0
    }

    pub fn get_current_native_handle(&self) -> Option<crate::subsystems::render::NativeTextureHandle> {
        // Get the current readable texture handle for DXGI sharing
        if let Ok(lock) = self.shared_textures.lock() {
            if let Some(ref textures) = *lock {
                let read_idx = textures.read_index.load(std::sync::atomic::Ordering::Acquire);
                if let Ok(handles_lock) = textures.native_handles.lock() {
                    if let Some(ref handles) = *handles_lock {
                        return Some(handles[read_idx].clone());
                    }
                }
            }
        }
        None
    }
}

impl Drop for HelioRenderer {
    fn drop(&mut self) {
        self.shutdown();
    }
}
