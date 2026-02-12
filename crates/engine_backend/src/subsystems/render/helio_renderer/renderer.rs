//! Main HelioRenderer struct and initialization logic
//! Matches BevyRenderer's API but uses blade-graphics + Helio features

use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};
use glam::{Vec3, Mat4};

use helio_core::{create_cube_mesh, create_sphere_mesh, create_plane_mesh, MeshBuffer, TextureManager};
use helio_render::{FpsCamera, FeatureRenderer, TransformUniforms};
use helio_features::FeatureRegistry;
use helio_feature_base_geometry::BaseGeometry;
use helio_feature_lighting::BasicLighting;
use helio_feature_materials::BasicMaterials;
use helio_feature_procedural_shadows::ProceduralShadows;
use helio_feature_bloom::Bloom;
use helio_feature_billboards::BillboardFeature;

use super::core::{CameraInput, RenderMetrics, GpuProfilerData, SharedGpuTextures};
use super::gizmo_types::{
    BevyGizmoType, BevyGizmoAxis, GizmoStateResource,
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
        shared_textures: Arc<Mutex<Option<SharedGpuTextures>>>,
        camera_input: Arc<Mutex<CameraInput>>,
        metrics: Arc<Mutex<RenderMetrics>>,
        gpu_profiler: Arc<Mutex<GpuProfilerData>>,
        _gizmo_state: Arc<Mutex<GizmoStateResource>>,
        _viewport_mouse_input: Arc<parking_lot::Mutex<ViewportMouseInput>>,
        shutdown: Arc<AtomicBool>,
        _game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    ) {
        profiling::profile_scope!("HelioRenderer::Run");
        tracing::info!("[HELIO] 🚀 Step 1/10: Starting headless renderer {}x{}", width, height);

        // Initialize blade-graphics context (headless)
        tracing::info!("[HELIO] 🚀 Step 2/10: Initializing blade-graphics context...");
        let context = Arc::new(unsafe {
            match blade_graphics::Context::init(blade_graphics::ContextDesc {
                presentation: false, // Headless - we'll handle presentation via DXGI
                validation: cfg!(debug_assertions),
                timing: false,
                capture: false,
                overlay: false,
                device_id: 0,
            }) {
                Ok(ctx) => {
                    tracing::info!("[HELIO] ✅ Step 2/10: blade-graphics context initialized!");
                    ctx
                }
                Err(e) => {
                    tracing::error!("[HELIO] ❌ FATAL: Failed to initialize blade-graphics: {:?}", e);
                    panic!("Cannot continue without graphics context");
                }
            }
        });

        // Create DXGI shared textures
        tracing::info!("[HELIO] 🚀 Step 3/10: Creating DXGI shared textures...");
        #[cfg(target_os = "windows")]
        let helio_shared_textures = match super::dxgi_textures::HelioSharedTextures::new(&context) {
            Ok(textures) => {
                tracing::info!("[HELIO] ✅ Step 3/10: DXGI shared textures created successfully!");
                
                // Store in shared state for GPUI access
                if let Ok(mut lock) = shared_textures.lock() {
                    *lock = Some(textures.to_shared_gpu_textures());
                    tracing::info!("[HELIO] ✅ Shared textures stored for GPUI access");
                } else {
                    tracing::error!("[HELIO] ❌ Failed to lock shared_textures mutex");
                }
                
                Some(textures)
            }
            Err(e) => {
                tracing::error!("[HELIO] ❌ Failed to create DXGI shared textures: {}", e);
                tracing::warn!("[HELIO] Continuing without texture sharing - viewport won't display");
                None
            }
        };

        #[cfg(not(target_os = "windows"))]
        let helio_shared_textures: Option<()> = None;

        // Create test meshes
        tracing::info!("[HELIO] 🚀 Step 4/10: Creating meshes...");
        let cube_mesh = MeshBuffer::from_mesh(&*context, "cube", &create_cube_mesh(1.0));
        let sphere_mesh = MeshBuffer::from_mesh(&*context, "sphere", &create_sphere_mesh(0.5, 32, 32));
        let plane_mesh = MeshBuffer::from_mesh(&*context, "plane", &create_plane_mesh(20.0, 20.0));
        tracing::info!("[HELIO] ✅ Step 4/10: Test meshes created");

        // Create TextureManager and load spotlight billboard texture
        tracing::info!("[HELIO] 🚀 Step 5/10: Creating TextureManager and loading textures...");
        let mut texture_manager = TextureManager::new(Arc::clone(&context));
        
        // Load spotlight.png for light billboards
        let spotlight_texture_id = match texture_manager.load_png("assets/editor_assets/spotlight.png") {
            Ok(id) => {
                tracing::info!("[HELIO] ✅ Loaded spotlight.png for light billboards");
                Some(id)
            }
            Err(e) => {
                tracing::warn!("[HELIO] ⚠️ Failed to load spotlight.png: {} - light billboards will not be visible", e);
                None
            }
        };
        
        let texture_manager = Arc::new(texture_manager);
        tracing::info!("[HELIO] ✅ Step 5/10: TextureManager created");

        // Initialize features with game scene lighting
        tracing::info!("[HELIO] 🚀 Step 6/10: Initializing rendering features...");
        let mut base_geometry = BaseGeometry::new();
        base_geometry.set_texture_manager(texture_manager.clone());
        let base_shader = base_geometry.shader_template().to_string();

        // Setup shadow system EXACTLY like lighting showcase
        let mut shadows = ProceduralShadows::new().with_ambient(0.0); // NO ambient light!
        shadows.set_texture_manager(texture_manager.clone());
        
        // Configure spotlight billboard icon
        if let Some(texture_id) = spotlight_texture_id {
            shadows.set_spotlight_icon(texture_id);
            tracing::info!("[HELIO] ✅ Spotlight billboard icon configured");
        }
        
        // Initial static lights (will be replaced by animated lights in loop)
        shadows.add_light(helio_feature_procedural_shadows::LightConfig {
            light_type: helio_feature_procedural_shadows::LightType::Spot {
                inner_angle: 25.0_f32.to_radians(),
                outer_angle: 40.0_f32.to_radians(),
            },
            position: Vec3::new(0.0, 8.0, 0.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 1.5,
            color: Vec3::new(1.0, 0.2, 0.2),
            attenuation_radius: 12.0,
            attenuation_falloff: 2.0,
        }).expect("Failed to add light");
        
        shadows.add_light(helio_feature_procedural_shadows::LightConfig {
            light_type: helio_feature_procedural_shadows::LightType::Point,
            position: Vec3::new(-4.0, 3.0, -4.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 1.2,
            color: Vec3::new(0.2, 1.0, 0.2),
            attenuation_radius: 10.0,
            attenuation_falloff: 2.5,
        }).expect("Failed to add light");
        
        shadows.add_light(helio_feature_procedural_shadows::LightConfig {
            light_type: helio_feature_procedural_shadows::LightType::Point,
            position: Vec3::new(4.0, 3.0, -4.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 1.2,
            color: Vec3::new(0.2, 0.2, 1.0),
            attenuation_radius: 10.0,
            attenuation_falloff: 2.5,
        }).expect("Failed to add light");
        
        tracing::info!("[HELIO] ✅ Shadow system configured with ambient=0.0 (pure black)");
        
        let mut billboards = BillboardFeature::new();
        billboards.set_texture_manager(texture_manager.clone());

        // Build feature registry
        let registry = FeatureRegistry::builder()
            .with_feature(base_geometry)
            .with_feature(BasicLighting::new())
            .with_feature(BasicMaterials::new())
            .with_feature(shadows)
            .with_feature(Bloom::new())
            .with_feature(billboards)
            .build();
        
        tracing::info!("[HELIO] ✅ Step 6/10: Feature registry built with 4 animated RGB lights!");

        // Create gizmo meshes
        tracing::info!("[HELIO] 🚀 Step 7/10: Creating gizmo meshes...");
        let arrow_mesh = MeshBuffer::from_mesh(
            &*context,
            "gizmo_arrow",
            &super::gizmos::create_arrow_mesh(1.0, 0.05, 0.1, 0.3),
        );
        let torus_mesh = MeshBuffer::from_mesh(
            &*context,
            "gizmo_torus",
            &super::gizmos::create_torus_mesh(1.0, 0.05, 32, 16),
        );
        tracing::info!("[HELIO] ✅ Step 7/10: Gizmo meshes created");

        // Use shared textures if available, otherwise create regular render target
        tracing::info!("[HELIO] 🚀 Step 8/10: Setting up render targets...");
        #[cfg(target_os = "windows")]
        let use_shared_textures = helio_shared_textures.is_some();
        
        #[cfg(not(target_os = "windows"))]
        let use_shared_textures = false;

        let fallback_render_target = if !use_shared_textures {
            tracing::warn!("[HELIO] Using fallback render target (no DXGI sharing)");
            Some(context.create_texture(blade_graphics::TextureDesc {
                name: "helio_render_target",
                format: blade_graphics::TextureFormat::Bgra8UnormSrgb,
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
            }))
        } else {
            tracing::info!("[HELIO] Using DXGI shared textures for rendering");
            None
        };
        tracing::info!("[HELIO] ✅ Step 8/10: Render targets configured");

        tracing::info!("[HELIO] 🚀 Step 9/10: Creating FeatureRenderer...");
        let mut renderer = FeatureRenderer::new(
            Arc::clone(&context),
            blade_graphics::TextureFormat::Bgra8UnormSrgb,
            width,
            height,
            registry,
            &base_shader,
        ).expect("Failed to create FeatureRenderer");
        tracing::info!("[HELIO] ✅ Step 9/10: FeatureRenderer created");

        tracing::info!("[HELIO] 🚀 Step 10/10: Creating command encoder...");
        let mut command_encoder = context.create_command_encoder(blade_graphics::CommandEncoderDesc {
            name: "helio_main",
            buffer_count: 2,
        });
        tracing::info!("[HELIO] ✅ Step 10/10: Command encoder created");

        // Camera setup
        let mut camera = FpsCamera::new(Vec3::new(0.0, 5.0, 15.0));
        camera.pitch = -20.0_f32.to_radians();

        let start_time = Instant::now();
        let mut last_frame_time = Instant::now();
        let mut frame_count: u64 = 0;

        tracing::info!("[HELIO] ✅✅✅ ALL INITIALIZATION COMPLETE - ENTERING RENDER LOOP ✅✅✅");

        // Main render loop
        while !shutdown.load(Ordering::Relaxed) {
            profiling::profile_scope!("Helio Frame");
            
            let now = Instant::now();
            let delta_time = (now - last_frame_time).as_secs_f32();
            last_frame_time = now;
            
            // Debug log every 60 frames
            if frame_count % 60 == 0 {
                tracing::debug!("[HELIO] Frame {} - FPS: {:.1}", frame_count, 1.0 / delta_time);
            }

            // Update camera from input
            if let Ok(mut input) = camera_input.lock() {
                // Apply movement with speed modifiers
                let speed_multiplier = if input.boost { 3.0 } else { 1.0 };
                let effective_speed = input.move_speed * speed_multiplier;
                
                camera.move_speed = effective_speed;
                camera.look_speed = input.look_sensitivity;
                
                // WASD movement
                camera.update_movement(input.forward, input.right, input.up, delta_time);
                
                // Mouse look - Use Helio's handle_mouse_delta method directly
                if input.mouse_delta_x.abs() > 0.001 || input.mouse_delta_y.abs() > 0.001 {
                    camera.handle_mouse_delta(input.mouse_delta_x, input.mouse_delta_y);
                    input.mouse_delta_x = 0.0; // Clear after applying
                    input.mouse_delta_y = 0.0;
                }
                
                // Middle-mouse pan
                if input.pan_delta_x.abs() > 0.001 || input.pan_delta_y.abs() > 0.001 {
                    let pan_speed = 0.01;
                    let right_offset = camera.right() * input.pan_delta_x * pan_speed;
                    let up_offset = Vec3::Y * -input.pan_delta_y * pan_speed;
                    camera.position += right_offset + up_offset;
                    input.pan_delta_x = 0.0; // Clear after applying
                    input.pan_delta_y = 0.0;
                }
                
                // Scroll wheel zoom
                if input.zoom_delta.abs() > 0.001 {
                    camera.position += camera.forward() * input.zoom_delta * 0.5;
                    input.zoom_delta = 0.0; // Clear after applying
                }
            }
            
            // === UPDATE DYNAMIC LIGHTS (RGB Multi-Light Dance) ===
            {
                profiling::profile_scope!("Update Lights");
                if let Some(shadows_feature) = renderer.registry_mut()
                    .get_feature_as_mut::<ProceduralShadows>("procedural_shadows") 
                {
                    shadows_feature.clear_lights();
                    
                    let time = (now - start_time).as_secs_f32();
                    
                    // Red spotlight (circling)
                    let r_angle = time * 0.8;
                    let _ = shadows_feature.add_light(helio_feature_procedural_shadows::LightConfig {
                        light_type: helio_feature_procedural_shadows::LightType::Spot {
                            inner_angle: 25.0_f32.to_radians(),
                            outer_angle: 40.0_f32.to_radians(),
                        },
                        position: Vec3::new(r_angle.cos() * 3.0, 7.0, r_angle.sin() * 3.0),
                        direction: Vec3::new(0.0, -1.0, 0.0),
                        intensity: 1.5,
                        color: Vec3::new(1.0, 0.1, 0.1),
                        attenuation_radius: 12.0,
                        attenuation_falloff: 2.0,
                    });
                    
                    // Green point light (circling opposite)
                    let g_angle = time * 1.2 + 2.0;
                    let _ = shadows_feature.add_light(helio_feature_procedural_shadows::LightConfig {
                        light_type: helio_feature_procedural_shadows::LightType::Point,
                        position: Vec3::new(g_angle.cos() * 5.0, 3.0, g_angle.sin() * 5.0),
                        direction: Vec3::new(0.0, -1.0, 0.0),
                        intensity: 1.3,
                        color: Vec3::new(0.1, 1.0, 0.1),
                        attenuation_radius: 10.0,
                        attenuation_falloff: 2.5,
                    });
                    
                    // Blue point light (different speed)
                    let b_angle = time * 1.0 + 4.0;
                    let _ = shadows_feature.add_light(helio_feature_procedural_shadows::LightConfig {
                        light_type: helio_feature_procedural_shadows::LightType::Point,
                        position: Vec3::new(b_angle.cos() * 4.0, 4.0, b_angle.sin() * 4.0),
                        direction: Vec3::new(0.0, -1.0, 0.0),
                        intensity: 1.3,
                        color: Vec3::new(0.1, 0.1, 1.0),
                        attenuation_radius: 10.0,
                        attenuation_falloff: 2.5,
                    });
                    
                    // Cyan point light (fast orbiting center)
                    let _ = shadows_feature.add_light(helio_feature_procedural_shadows::LightConfig {
                        light_type: helio_feature_procedural_shadows::LightType::Point,
                        position: Vec3::new(
                            (time * 1.5).cos() * 2.0,
                            2.0,
                            (time * 1.5).sin() * 2.0,
                        ),
                        direction: Vec3::new(0.0, -1.0, 0.0),
                        intensity: 0.8,
                        color: Vec3::new(0.3, 1.0, 1.0),
                        attenuation_radius: 6.0,
                        attenuation_falloff: 3.0,
                    });
                }
            }

            // Start frame
            #[cfg(target_os = "windows")]
            let render_target = if let Some(ref shared_tex) = helio_shared_textures {
                // Use current write buffer from shared textures
                shared_tex.get_write_texture()
            } else {
                fallback_render_target.unwrap()
            };
            
            #[cfg(not(target_os = "windows"))]
            let render_target = fallback_render_target.unwrap();
            
            command_encoder.start();
            command_encoder.init_texture(render_target);

            let aspect = width as f32 / height as f32;
            let camera_uniforms = camera.build_camera_uniforms(60.0, aspect);

            // === LIGHTING SHOWCASE SCENE - EXACT COPY ===
            // Ground plane
            let ground = Mat4::from_translation(Vec3::new(0.0, -0.1, 0.0))
                * Mat4::from_scale(Vec3::new(1.5, 1.0, 1.5));
            let mut meshes = vec![(TransformUniforms::from_matrix(ground), &plane_mesh)];

            let elapsed = (now - start_time).as_secs_f32();

            // Central rotating pillar of spheres
            for i in 0..5 {
                let height = i as f32 * 1.5;
                let angle = elapsed * 0.5 + i as f32 * 0.6;
                let radius = 2.0 + (elapsed * 0.3 + i as f32).sin() * 0.5;
                let t = Mat4::from_translation(Vec3::new(
                    angle.cos() * radius,
                    height + 1.0,
                    angle.sin() * radius,
                )) * Mat4::from_scale(Vec3::splat(
                    0.8 + (elapsed * 2.0 + i as f32).sin().abs() * 0.3,
                ));
                meshes.push((TransformUniforms::from_matrix(t), &sphere_mesh));
            }

            // Orbiting cubes
            for i in 0..8 {
                let orbit_angle = elapsed * 0.8 + (i as f32 / 8.0) * std::f32::consts::TAU;
                let height = 2.0 + (elapsed * 1.5 + i as f32).sin() * 2.0;
                let rs = 1.0 + i as f32 * 0.2;
                let t = Mat4::from_translation(Vec3::new(
                    orbit_angle.cos() * 6.0,
                    height,
                    orbit_angle.sin() * 6.0,
                )) * Mat4::from_rotation_y(elapsed * rs)
                    * Mat4::from_rotation_x(elapsed * rs * 0.7)
                    * Mat4::from_scale(Vec3::splat(0.6));
                meshes.push((TransformUniforms::from_matrix(t), &cube_mesh));
            }

            // Dancing spheres on the ground
            for i in 0..12 {
                let dance_angle = (i as f32 / 12.0) * std::f32::consts::TAU;
                let dance_radius = 4.0 + (elapsed * 0.5).sin() * 1.0;
                let bounce = ((elapsed * 3.0 + i as f32).sin().abs() * 2.0 + 0.5).max(0.5);
                let t = Mat4::from_translation(Vec3::new(
                    dance_angle.cos() * dance_radius,
                    bounce,
                    dance_angle.sin() * dance_radius,
                )) * Mat4::from_scale(Vec3::splat(
                    0.4 + (elapsed + i as f32).cos().abs() * 0.2,
                ));
                meshes.push((TransformUniforms::from_matrix(t), &sphere_mesh));
            }

            // Spinning double helix of cubes
            for i in 0..16 {
                let helix_height = i as f32 * 0.6;
                let a1 = elapsed * 2.0 + i as f32 * 0.4;
                let a2 = a1 + std::f32::consts::PI;
                let hr = 2.5;
                for a in [a1, a2] {
                    let t = Mat4::from_translation(Vec3::new(
                        a.cos() * hr,
                        helix_height + 0.5,
                        a.sin() * hr,
                    )) * Mat4::from_rotation_y(elapsed * 3.0)
                        * Mat4::from_scale(Vec3::splat(0.3));
                    meshes.push((TransformUniforms::from_matrix(t), &cube_mesh));
                }
            }

            // Add editor-placed objects from GameState if available
            if let Some(ref game_state_arc) = _game_thread_state {
                if let Ok(game_state) = game_state_arc.lock() {
                    for obj in &game_state.objects {
                        if !obj.active {
                            continue;
                        }
                        
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
                        
                        let transform_uniforms = TransformUniforms::from_matrix(translation * rotation * scale);
                        meshes.push((transform_uniforms, &sphere_mesh));
                    }
                }
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
                    
                    // Gizmo rendering disabled
                    /*
                    use super::gizmos::{GizmoType, GizmoAxis, create_gizmo_arrow_transform, create_gizmo_torus_transform};
                    
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
                    */
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

            // Swap buffers for double-buffering
            #[cfg(target_os = "windows")]
            if let Some(ref shared_tex) = helio_shared_textures {
                shared_tex.swap_buffers();
                
                // Debug log occasionally
                if frame_count % 120 == 0 {
                    tracing::debug!("[HELIO] Buffer swapped, frame {}", frame_count);
                }
            }

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

            // No sleep - run at full display refresh rate!
            // GPUI can read from the shared texture whenever it wants,
            // we just keep rendering as fast as possible.
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

    pub fn get_current_native_handle(&self) -> Option<gpui::GpuTextureHandle> {
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
