/// Helio Surface Component — Direct WGPU integration for Level Editor
///
/// This component integrates Helio renderer with GPUI using a WgpuSurfaceHandle,
/// following the exact pattern from WGPUI's wgpu_surface.rs example.
///
/// Key design:
/// - WgpuSurfaceHandle provides device/queue on every frame
/// - Helio renderer initialized lazily on first render  
/// - Direct rendering to back buffer view
/// - Camera and scene controlled from level editor state

use gpui::*;
use helio::*;
use std::sync::Arc;
use std::time::Instant;

use crate::level_editor::ui::state::LevelEditorState;

/// Helio render state (initialized once, reused across frames)
pub struct HelioRenderState {
    pub renderer: Renderer,
    pub last_update: Instant,
    pub width: u32,
    pub height: u32,
}

/// Helio Surface Element — renders 3D viewport using Helio
pub struct HelioSurface {
    surface: WgpuSurfaceHandle,
    state: Option<HelioRenderState>,
    // Level editor state for camera/scene control
    editor_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    // FPS tracking
    frame_count: u32,
    last_fps_update: Instant,
    display_fps: f64,
}

impl HelioSurface {
    /// Create a new Helio surface
    pub fn new(
        window: &mut Window,
        width: u32,
        height: u32,
        editor_state: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let surface = window
            .create_wgpu_surface(width, height, wgpu::TextureFormat::Rgba8UnormSrgb)
            .expect("WgpuSurface not supported on this platform");

        let now = Instant::now();
        Self {
            surface,
            state: None,
            editor_state,
            frame_count: 0,
            last_fps_update: now,
            display_fps: 0.0,
        }
    }

    /// Get the surface handle (for external access if needed)
    pub fn surface(&self) -> &WgpuSurfaceHandle {
        &self.surface
    }

    /// Get current FPS
    pub fn fps(&self) -> f64 {
        self.display_fps
    }

    /// Initialize Helio renderer with external device/queue
    fn init_helio(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        editor_state: &Arc<parking_lot::RwLock<LevelEditorState>>,
    ) -> HelioRenderState {
        let config = RendererConfig::new(width, height, format);
        let mut renderer = Renderer::new_with_external_device(device, queue, config);

        // Populate scene from level editor state
        let state = editor_state.read();
        
        // Create materials
        let default_mat = renderer.scene_mut().insert_material(GpuMaterial {
            base_color: [0.8, 0.8, 0.8, 1.0],
            emissive: [0.0, 0.0, 0.0, 0.0],
            roughness_metallic: [0.5, 0.0, 1.5, 0.5],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: 0,
            _pad: 0,
        });

        // Add sky
        renderer.scene_mut().insert_actor(SceneActor::Sky(
            SkyActor::new().with_clouds(VolumetricClouds {
                coverage: 0.4,
                density: 0.5,
                base: 1000.0,
                top: 2000.0,
                wind_x: 1.0,
                wind_z: 0.5,
                speed: 1.0,
                skylight_intensity: 0.25,
            })
        ));

        // Populate objects from scene database
        if let Some(scene_db) = &state.scene_db {
            for entity in scene_db.entities() {
                // TODO: Convert scene_db entities to Helio objects
                // For now, create a placeholder cube
                let mesh = renderer.scene_mut().insert_actor(SceneActor::mesh(
                    create_cube_mesh([0.0, 0.5, 0.0], 0.5)
                )).as_mesh().unwrap();
                
                let transform = glam::Mat4::IDENTITY;
                let _ = renderer.scene_mut().insert_actor(SceneActor::object(
                    ObjectDescriptor {
                        mesh,
                        material: default_mat,
                        transform,
                        bounds: [0.0, 0.5, 0.0, 0.5],
                        flags: 0,
                        groups: GroupMask::NONE,
                        movability: Some(Movability::Movable),
                    }
                ));
            }
        }

        // Add default directional light (sun)
        renderer.scene_mut().insert_actor(SceneActor::light(GpuLight {
            position_range: [0.0, 0.0, 0.0, f32::MAX],
            direction_outer: [-0.3, -1.0, -0.5, 0.0],
            color_intensity: [1.0, 0.95, 0.9, 0.5],
            shadow_index: 0,
            light_type: LightType::Directional as u32,
            inner_angle: 0.0,
            _pad: 0,
        }));

        // Add ambient point lights
        renderer.scene_mut().insert_actor(SceneActor::light(GpuLight {
            position_range: [3.0, 2.0, 3.0, 10.0],
            direction_outer: [0.0, 0.0, -1.0, 0.0],
            color_intensity: [0.8, 0.9, 1.0, 2.0],
            shadow_index: 0,
            light_type: LightType::Point as u32,
            inner_angle: 0.0,
            _pad: 0,
        }));

        renderer.scene_mut().insert_actor(SceneActor::light(GpuLight {
            position_range: [-3.0, 2.0, -3.0, 10.0],
            direction_outer: [0.0, 0.0, -1.0, 0.0],
            color_intensity: [1.0, 0.8, 0.7, 2.0],
            shadow_index: 0,
            light_type: LightType::Point as u32,
            inner_angle: 0.0,
            _pad: 0,
        }));

        // Set ambient lighting
        renderer.set_ambient([0.15, 0.18, 0.25], 0.1);

        HelioRenderState {
            renderer,
            last_update: Instant::now(),
            width,
            height,
        }
    }

    /// Render one frame
    fn render_frame(&mut self, cx: &mut Context<Self>) {
        if let Some((view, (width, height))) = self.surface.back_view_with_size() {
            // Get device/queue from surface (fresh instances each frame as per WGPUI pattern)
            let device = Arc::new(self.surface.device().clone());
            let queue = Arc::new(self.surface.queue().clone());
            let format = self.surface.format();

            // Lazy initialization of Helio renderer
            let state = self.state.get_or_insert_with(|| {
                Self::init_helio(device.clone(), queue.clone(), width, height, format, &self.editor_state)
            });

            // Handle resize
            if state.width != width || state.height != height {
                state.renderer.set_render_size(width, height);
                state.width = width;
                state.height = height;
            }

            // Get camera from level editor state
            let editor_state = self.editor_state.read();
            let camera_pos = editor_state.camera_state.position;
            let camera_yaw = editor_state.camera_state.yaw;
            let camera_pitch = editor_state.camera_state.pitch;

            // Compute camera forward vector
            let (sy, cy) = camera_yaw.sin_cos();
            let (sp, cp) = camera_pitch.sin_cos();
            let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
            
            // Create camera
            let aspect = width as f32 / height.max(1) as f32;
            let camera = Camera::perspective_look_at(
                camera_pos,
                camera_pos + forward,
                glam::Vec3::Y,
                std::f32::consts::FRAC_PI_4,
                aspect,
                0.1,
                1000.0,
            );

            // Render!
            if let Err(e) = state.renderer.render(&camera, &view) {
                tracing::error!("[HelioSurface] Render error: {:?}", e);
            }

            drop(view);
            self.surface.swap_buffers();

            // Update FPS
            let now = Instant::now();
            self.frame_count = self.frame_count.wrapping_add(1);
            if now.duration_since(self.last_fps_update) >= std::time::Duration::from_secs(1) {
                self.display_fps = self.frame_count as f64;
                self.frame_count = 0;
                self.last_fps_update = now;
            }
        }

        // Request next frame
        cx.notify();
    }
}

impl Render for HelioSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render the frame
        self.render_frame(cx);

        // Return the UI element
        div()
            .flex()
            .w_full()
            .h_full()
            .child(
                wgpu_surface(self.surface.clone())
                    .absolute()
                    .inset_0()
            )
    }
}

// ── Mesh helpers ────────────────────────────────────────────────────────────

fn create_cube_mesh(center: [f32; 3], half_extent: f32) -> MeshUpload {
    let c = glam::Vec3::from_array(center);
    let e = half_extent;
    let corners = [
        c + glam::Vec3::new(-e, -e,  e),
        c + glam::Vec3::new( e, -e,  e),
        c + glam::Vec3::new( e,  e,  e),
        c + glam::Vec3::new(-e,  e,  e),
        c + glam::Vec3::new(-e, -e, -e),
        c + glam::Vec3::new( e, -e, -e),
        c + glam::Vec3::new( e,  e, -e),
        c + glam::Vec3::new(-e,  e, -e),
    ];
    
    let faces: [([usize; 4], [f32; 3], [f32; 3]); 6] = [
        ([0, 1, 2, 3], [0.0,  0.0,  1.0], [ 1.0, 0.0,  0.0]), // front
        ([5, 4, 7, 6], [0.0,  0.0, -1.0], [-1.0, 0.0,  0.0]), // back
        ([4, 0, 3, 7], [-1.0, 0.0,  0.0], [ 0.0, 0.0,  1.0]), // left
        ([1, 5, 6, 2], [ 1.0, 0.0,  0.0], [ 0.0, 0.0, -1.0]), // right
        ([3, 2, 6, 7], [0.0,  1.0,  0.0], [ 1.0, 0.0,  0.0]), // top
        ([4, 5, 1, 0], [0.0, -1.0,  0.0], [ 1.0, 0.0,  0.0]), // bottom
    ];
    
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    
    for (face_index, (quad, normal, tangent)) in faces.iter().enumerate() {
        let base = (face_index * 4) as u32;
        let uv = [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (i, &corner_index) in quad.iter().enumerate() {
            vertices.push(PackedVertex::from_components(
                corners[corner_index].to_array(),
                *normal,
                uv[i],
                *tangent,
                1.0,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    
    MeshUpload { vertices, indices }
}
