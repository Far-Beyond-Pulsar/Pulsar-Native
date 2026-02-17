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
use helio_feature_skies::HelioSkies;

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
    /// Shared scene database - the renderer reads from this directly each frame.
    /// The same Arc is held by all UI panels; writes are immediately visible to the renderer.
    pub scene_db: Arc<crate::scene::SceneDb>,
    shutdown: Arc<AtomicBool>,
    _render_thread: Option<std::thread::JoinHandle<()>>,
}

impl HelioRenderer {
    pub async fn new(width: u32, height: u32) -> Self {
        Self::new_with_all(width, height, None, Arc::new(crate::scene::SceneDb::new()), None).await
    }

    pub async fn new_with_game_thread(
        width: u32,
        height: u32,
        game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
        scene_db: Arc<crate::scene::SceneDb>,
    ) -> Self {
        Self::new_with_all(width, height, game_thread_state, scene_db, None).await
    }

    pub async fn new_with_all(
        width: u32,
        height: u32,
        game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
        scene_db: Arc<crate::scene::SceneDb>,
        physics_query: Option<Arc<crate::services::PhysicsQueryService>>,
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
        let scene_db_clone = scene_db.clone();
        let shutdown_clone = shutdown.clone();
        let game_thread_clone = game_thread_state.clone();
        let physics_query_clone = physics_query.clone();

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
                    scene_db_clone,
                    shutdown_clone,
                    game_thread_clone,
                    physics_query_clone,
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
            scene_db,
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
        scene_db: Arc<crate::scene::SceneDb>,
        shutdown: Arc<AtomicBool>,
        _game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
        physics_query: Option<Arc<crate::services::PhysicsQueryService>>,
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
        
        // Create large sky sphere (500 unit radius for far-distance sky rendering)
        let sky_sphere = MeshBuffer::from_mesh(&*context, "sky_sphere", &create_sphere_mesh(500.0, 32, 32));
        tracing::info!("[HELIO] ✅ Step 4/10: Test meshes created (including sky sphere)");

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
            .with_feature(HelioSkies::new())
            .build();
        tracing::info!("[HELIO] ✅ Helio Skies feature added to registry");
        
        // Create gizmo renderer separately (not part of feature registry)
        let mut gizmo_renderer = super::gizmo_feature::GizmoRenderer::new(Arc::clone(&scene_db));
        gizmo_renderer.init(&context, blade_graphics::TextureFormat::Bgra8UnormSrgb, blade_graphics::TextureFormat::Depth32Float);
        tracing::info!("[HELIO] ✅ Gizmo renderer initialized");

        // Create debug line renderer for visualizing raycasts
        let mut debug_line_renderer = super::debug_line_renderer::DebugLineRenderer::new();
        debug_line_renderer.init(&context, blade_graphics::TextureFormat::Bgra8UnormSrgb, blade_graphics::TextureFormat::Depth32Float);
        tracing::info!("[HELIO] ✅ Debug line renderer initialized");

        tracing::info!("[HELIO] ✅ Step 6/10: Feature registry built with gizmo overlay feature!");

        // Use shared textures if available, otherwise create regular render target
        tracing::info!("[HELIO] 🚀 Step 7/10: Setting up render targets...");
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

        // Create depth texture for the render pipeline
        tracing::info!("[HELIO] 🚀 Step 8.5/10: Creating depth texture...");
        let depth_texture = context.create_texture(blade_graphics::TextureDesc {
            name: "helio_depth_buffer",
            format: blade_graphics::TextureFormat::Depth32Float,
            size: blade_graphics::Extent {
                width,
                height,
                depth: 1,
            },
            dimension: blade_graphics::TextureDimension::D2,
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            usage: blade_graphics::TextureUsage::TARGET,
            external: None,
        });
        tracing::info!("[HELIO] ✅ Step 8.5/10: Depth texture created");

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
            
            // Process mouse click for object selection with physics raycasting
            if let Some(mut mouse_input) = _viewport_mouse_input.try_lock() {
                if mouse_input.left_clicked {
                    println!("[RENDERER] 🖱  left_clicked fired — physics_query present: {}", physics_query.is_some());
                    mouse_input.left_clicked = false; // Clear click flag
                    
                    // Convert normalized screen coords to world ray
                    let ndc_x = mouse_input.mouse_pos.x * 2.0 - 1.0;
                    let ndc_y = 1.0 - mouse_input.mouse_pos.y * 2.0; // Flip Y
                    
                    // Create ray from camera through mouse position
                    let aspect = width as f32 / height as f32;
                    let fov = 60.0_f32.to_radians();
                    let tan_half_fov = (fov * 0.5).tan();
                    
                    let ray_dir_cam = Vec3::new(
                        ndc_x * aspect * tan_half_fov,
                        ndc_y * tan_half_fov,
                        -1.0,
                    ).normalize();
                    
                    // Transform ray direction to world space
                    let cam_forward = camera.forward();
                    let cam_right = camera.right();
                    let cam_up = cam_right.cross(cam_forward).normalize(); // Compute up vector
                    
                    // Negate forward because camera.forward() points down -Z, but we computed ray_dir_cam.z = -1.0
                    // So we want: cam_forward * -ray_dir_cam.z = cam_forward * -(-1.0) = cam_forward
                    // Wait no - if camera is at z=23 looking at origin, forward should be negative Z
                    // So we just use the forward direction as-is
                    let ray_dir_world = (
                        cam_right * ray_dir_cam.x +
                        cam_up * ray_dir_cam.y +
                        cam_forward * ray_dir_cam.z  // ray_dir_cam.z is already -1.0
                    ).normalize();
                    
                    let ray_origin = camera.position;
                    
                    tracing::info!("[VIEWPORT] 🎯 Camera at [{:.2}, {:.2}, {:.2}], forward=[{:.3}, {:.3}, {:.3}]",
                        ray_origin.x, ray_origin.y, ray_origin.z,
                        cam_forward.x, cam_forward.y, cam_forward.z);
                    tracing::info!("[VIEWPORT] 🎯 Ray dir: [{:.3}, {:.3}, {:.3}] (should point toward scene)",
                        ray_dir_world.x, ray_dir_world.y, ray_dir_world.z);
                    
                    // Use physics raycast if available, otherwise fallback to simple intersection
                    if let Some(ref pq) = physics_query {
                        tracing::info!("[VIEWPORT] Using physics raycast");
                        // Push the line start slightly in front of the camera so the
                        // start vertex is never at clip_w=0 (camera origin in view space).
                        let line_start = (ray_origin + ray_dir_world * 0.5).to_array();

                        if let Some(hit) = pq.raycast(ray_origin, ray_dir_world, 1000.0) {
                            tracing::info!("[VIEWPORT] 🎯 Object selected via physics raycast: {} at distance {:.2}", hit.object_id, hit.distance);
                            scene_db.select_object(Some(hit.object_id.clone()));
                            // Solid red from just-in-front-of-camera → hit point
                            debug_line_renderer.add_line(
                                line_start,
                                hit.point.to_array(),
                                [1.0, 0.0, 0.0, 1.0],
                                120,
                            );
                        } else {
                            tracing::info!("[VIEWPORT] ❌ No object hit by physics raycast");
                            scene_db.select_object(None);
                            // Orange miss ray capped at 50 units
                            let miss_end = ray_origin + ray_dir_world * 50.0;
                            debug_line_renderer.add_line(
                                line_start,
                                miss_end.to_array(),
                                [1.0, 0.4, 0.0, 1.0],
                                120,
                            );
                        }
                    } else {
                        println!("[RENDERER] ⚠️  Using fallback raycast (no physics query service)");
                        tracing::info!("[VIEWPORT] Using fallback raycast (no physics)");
                        // Fallback: Simple sphere intersection test
                        let mut closest_hit: Option<(String, f32)> = None;
                        let mut closest_dist = f32::MAX;

                        scene_db.for_each_entry(|entry| {
                            if !entry.is_visible() {
                                return;
                            }

                            let obj_pos = Vec3::new(
                                entry.get_position()[0],
                                entry.get_position()[1],
                                entry.get_position()[2],
                            );

                            let to_obj = obj_pos - ray_origin;
                            let proj = to_obj.dot(ray_dir_world);

                            if proj > 0.0 {
                                let closest_point = ray_origin + ray_dir_world * proj;
                                let dist_to_ray = (obj_pos - closest_point).length();

                                let scale = entry.get_scale();
                                let radius = (scale[0] + scale[1] + scale[2]) / 3.0 * 0.707;

                                if dist_to_ray < radius {
                                    if closest_hit.is_none() || proj < closest_hit.as_ref().unwrap().1 {
                                        closest_hit = Some((entry.id.clone(), proj));
                                        closest_dist = proj;
                                    }
                                }
                            }
                        });

                        let line_start = (ray_origin + ray_dir_world * 0.5).to_array();

                        if let Some((object_id, dist)) = closest_hit {
                            tracing::info!("[VIEWPORT] 🎯 Object selected via fallback raycast: {}", object_id);
                            scene_db.select_object(Some(object_id));
                            let hit_point = ray_origin + ray_dir_world * dist;
                            debug_line_renderer.add_line(
                                line_start,
                                hit_point.to_array(),
                                [1.0, 0.0, 0.0, 1.0],
                                120,
                            );
                        } else {
                            tracing::info!("[VIEWPORT] ❌ No object hit by fallback raycast");
                            scene_db.select_object(None);
                            let miss_end = ray_origin + ray_dir_world * 50.0;
                            debug_line_renderer.add_line(
                                line_start,
                                miss_end.to_array(),
                                [1.0, 0.4, 0.0, 1.0],
                                120,
                            );
                        }
                    }
                }
            }
            
            // Sync scene objects with physics colliders (for raycasting)
            if let Some(ref pq) = physics_query {
                // Only sync every 60 frames to avoid overhead
                if frame_count % 60 == 0 {
                    tracing::info!("[RENDER] Syncing scene with physics (frame {})", frame_count);
                    pq.sync_from_scene(&scene_db);
                }
            } else {
                if frame_count % 300 == 0 {
                    tracing::warn!("[RENDER] ⚠️  No PhysicsQueryService - viewport picking will use fallback");
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

            // === SCENE DATABASE - render what's actually in the scene ===
            // Sky sphere FIRST - positioned at camera, very large
            let sky_transform = Mat4::from_translation(camera.position);
            let mut meshes = vec![(TransformUniforms::from_matrix(sky_transform), &sky_sphere)];
            
            // Ground plane (always present for orientation)
            let ground = Mat4::from_translation(Vec3::new(0.0, -0.01, 0.0))
                * Mat4::from_scale(Vec3::new(20.0, 1.0, 20.0));
            meshes.push((TransformUniforms::from_matrix(ground), &plane_mesh));

            // Read all scene objects lock-free via DashMap + atomic transforms
            scene_db.for_each_entry(|entry| {
                use crate::scene::{ObjectType, MeshType};
                if !entry.is_visible() {
                    return;
                }
                let mat = entry.to_mat4();
                let tu = TransformUniforms::from_matrix(mat);
                match entry.object_type {
                    ObjectType::Mesh(MeshType::Cube) => {
                        meshes.push((tu, &cube_mesh));
                    }
                    ObjectType::Mesh(MeshType::Sphere) => {
                        meshes.push((tu, &sphere_mesh));
                    }
                    ObjectType::Mesh(MeshType::Plane) => {
                        meshes.push((tu, &plane_mesh));
                    }
                    // Cylinder falls back to cube for now; Camera/Light/Empty etc. not rendered as meshes
                    ObjectType::Mesh(MeshType::Cylinder) | ObjectType::Mesh(MeshType::Custom) => {
                        meshes.push((tu, &cube_mesh));
                    }
                    _ => {}
                }
            });

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

            // Render gizmos as overlay (after main scene, using InitOp::Load to preserve scene)
            let depth_view = context.create_texture_view(
                depth_texture,
                blade_graphics::TextureViewDesc {
                    name: "helio_depth_view",
                    format: blade_graphics::TextureFormat::Depth32Float,
                    dimension: blade_graphics::ViewDimension::D2,
                    subresources: &blade_graphics::TextureSubresources::default(),
                },
            );
            
            gizmo_renderer.render(
                &mut command_encoder,
                render_target_view,
                depth_view,
                camera_uniforms.view_proj,
                camera.position.to_array(),
            );

            // Render debug lines (raycasts) as overlay on top of everything
            debug_line_renderer.render(
                &mut command_encoder,
                render_target_view,
                camera_uniforms.view_proj,
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
