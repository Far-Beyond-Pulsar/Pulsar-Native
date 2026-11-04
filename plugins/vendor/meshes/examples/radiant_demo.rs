// Radiant Material System Demo
// Demonstrates all three tiers of the Radiant material system:
//   Tier 1 — Feature flags on the default PBR uber-shader
//   Tier 2 — Hand-authored surface templates (clear coat, SSS, anisotropic, skin)
//   Tier 3 — Full custom template (iridescent thin-film surface)
//
// The sky and GI left Helio with their plugins (Pulsar `plugins/vendor/sky`,
// `plugins/vendor/gi`): no atmosphere or GI probes here.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    GpuMaterial, Renderer, RendererBuilder, RendererConfig,
};
use helio_components::MaterialComponent;
use helio_mats::{
    MATERIAL_CLASS_ANISOTROPIC, MATERIAL_CLASS_CLEAR_COAT, MATERIAL_CLASS_SKIN,
    MATERIAL_CLASS_SUBSURFACE,
};
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    directional_light, make_material, new_scene_db_with_gpu_mirror, plane_mesh, point_light,
    scene_db_handle, spawn_light, spawn_material, spawn_mesh, spawn_object, sphere_mesh,
    update_light,
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

mod v3_demo_common;

const LOOK_SENS: f32 = 0.002;
const FLY_SPEED: f32 = 10.0;
const DRAG: f32 = 6.0;

struct App {
    state: Option<AppState>,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    renderer: Renderer,
    scene_db: SceneDb,
    last_frame: Instant,
    cam_pos: Vec3,
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),
    animated_iri_id: Entity,
    crystal_mat_id: Entity,
    aniso_mat_id: Entity,
    sun_light_id: Entity,
    key_light_id: Entity,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("Helio Radiant Material Demo")
            .with_inner_size(winit::dpi::PhysicalSize::new(1600, 900));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());

        let size = window.inner_size();
        let mut graphics = pollster::block_on(helio::HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            helio::HelioGraphicsConfig {
                vr_support: true,
                application_name: "Radiant Demo".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let format = graphics.surface_config.format;
        let surface_format = format;
        let alpha_mode = graphics.surface_config.alpha_mode;

        let mut config = RendererConfig::new(size.width, size.height, surface_format);
        config.enable_ssr = true;
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        // The materials' template registries: registered into below, read by
        // the G-buffer drawer (shared handles, so registration order does not
        // matter).
        let templates = helio_mats::MaterialTemplates::default();
        let mut renderer = RendererBuilder::new(config, scene_db_handle(&scene_db))
            .with_feature(templates.clone())
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build_with_graphics_context(&mut graphics)
            .expect("Helio renderer");
        let surface = graphics.surface;

        // ── Register iridescent template (Tier 3) ───────────────────────────

        let iridescent_wgsl = include_str!("shaders/radiant_iridescent.wgsl");
        let iridescent_class = templates
            .opaque
            .write()
            .unwrap()
            .register_str("iridescent", iridescent_wgsl.to_string());
        log::info!(
            "[RADIANT] Iridescent template registered as class {}",
            iridescent_class
        );

        // ── Register opal template ───────────────────────────────────────────

        let opal_wgsl = helio_mats::templates::OPAL;
        let opal_class = templates
            .opaque
            .write()
            .unwrap()
            .register_partial_str("opal", opal_wgsl.to_string());
        log::info!("[RADIANT] Opal template registered as class {}", opal_class);

        // ── Register glass template ─────────────────────────────────────────

        let glass_wgsl = helio_mats::templates::GLASS;
        let glass_class = templates
            .opaque
            .write()
            .unwrap()
            .register_partial_str("glass", glass_wgsl.to_string());
        log::info!(
            "[RADIANT] Glass (gbuffer) template registered as class {}",
            glass_class
        );

        // Register transparent version at the gbuffer glass class ID so the
        // transparent pass finds the glass shader instead of falling back to default.
        let glass_transparent_wgsl = helio_mats::templates::GLASS_TRANSPARENT;
        let glass_transparent_src = templates
            .transparent
            .write()
            .unwrap()
            .compose_transparent_override(glass_transparent_wgsl);
        templates
            .transparent
            .write()
            .unwrap()
            .register_str_at(glass_class, "glass_transparent", glass_transparent_src);
        log::info!(
            "[RADIANT] Glass (transparent) template registered as class {}",
            glass_class
        );

        // ── Register water template ─────────────────────────────────────────

        let water_wgsl = helio_mats::templates::WATER;
        let water_class = templates
            .opaque
            .write()
            .unwrap()
            .register_partial_str("water", water_wgsl.to_string());
        log::info!(
            "[RADIANT] Water (gbuffer) template registered as class {}",
            water_class
        );

        // Also register a transparent water template so the transparent pass
        // renders it with real alpha blending instead of the fixed overlay.
        let water_transparent_wgsl = helio_mats::templates::WATER_TRANSPARENT;
        let water_transparent_class = templates
            .transparent
            .write()
            .unwrap()
            .register_transparent_partial_str(
                "water_transparent",
                water_transparent_wgsl.to_string(),
            );
        log::info!(
            "[RADIANT] Water (transparent) template registered as class {}",
            water_transparent_class
        );

        // Tier-2 templates (clear_coat, subsurface, anisotropic, skin) are
        // auto-registered with their MATERIAL_CLASS_* IDs at RadianTemplate::new().
        //
        // The "Tier 2 graph snippet" showcase material (custom per-material
        // WGSL emissive-pulse injection via `Scene::radiant_graphs`) has no
        // SceneDB replacement -- that hash-keyed runtime snippet registry
        // was renderer/Scene-owned and was not carried over. Every other
        // tier's material still applies: it's now authored directly on
        // `MaterialComponent`'s own `material_class`/`class_params` fields
        // (what `set_material_class`/`update_material_class_params` used to
        // set through the removed `Scene`), so it needs no runtime registry.

        // ── Materials ───────────────────────────────────────────────────────

        let make_mat = make_material;

        // Tier 1a: Gold metallic — broad sharp highlight
        let gold_mat = spawn_material(
            &mut scene_db.world,
            make_mat([1.0, 0.75, 0.2, 1.0], 0.15, 1.0, [0.0; 3], 0.0),
        );

        // Tier 1b: Rough red plastic — soft matte diffuse
        let plastic_mat = spawn_material(
            &mut scene_db.world,
            make_mat([0.9, 0.12, 0.08, 1.0], 0.85, 0.0, [0.0; 3], 0.0),
        );

        // Tier 2: Clear coat — dark base with bright, sharp coated specular
        let coat_component: MaterialComponent = GpuMaterial {
            base_color: [0.01, 0.01, 0.02, 1.0],
            emissive: [0.0; 4],
            roughness_metallic: [0.3, 0.0, 1.5, 0.0],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: 0,
            material_class: MATERIAL_CLASS_CLEAR_COAT,
            class_params: [1.0, 0.01, 0.0, 0.0],
        }
        .into();
        let coat_mat = spawn_material(&mut scene_db.world, coat_component);

        // Tier 2: Crystal/gemstone — SSS with rim-transmission glow
        let crystal_component: MaterialComponent = GpuMaterial {
            base_color: [0.98, 0.95, 0.92, 1.0],
            emissive: [0.0; 4],
            roughness_metallic: [0.01, 0.0, 2.42, 0.0],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: 0,
            material_class: MATERIAL_CLASS_SUBSURFACE,
            class_params: [0.2, 0.5, 0.9, 3.0],
        }
        .into();
        let crystal_mat = spawn_material(&mut scene_db.world, crystal_component);

        // Tier 2: Brushed metal — stretched anisotropic highlight
        let mut aniso = make_mat([0.75, 0.6, 0.4, 1.0], 0.2, 1.0, [0.0; 3], 0.0);
        aniso.material_class = MATERIAL_CLASS_ANISOTROPIC;
        aniso.class_params = [0.95, 0.0, 0.0, 0.0];
        let aniso_mat = spawn_material(&mut scene_db.world, aniso);

        // Tier 2: Skin — F0=0.028 dielectric with SSS
        let mut skin = make_mat([0.82, 0.58, 0.48, 1.0], 0.35, 0.0, [0.0; 3], 0.0);
        skin.material_class = MATERIAL_CLASS_SKIN;
        skin.class_params = [0.75, 0.1, 0.05, 4.0];
        let skin_mat = spawn_material(&mut scene_db.world, skin);

        // Tier 3: Iridescent static
        let mut iri = make_mat([0.6, 0.6, 0.8, 1.0], 0.12, 0.8, [0.0; 3], 0.0);
        iri.material_class = iridescent_class;
        let iri_mat = spawn_material(&mut scene_db.world, iri);

        // Tier 3: Iridescent animated
        let mut anim_iri = make_mat([0.5, 0.5, 0.6, 1.0], 0.15, 0.7, [0.0; 3], 0.0);
        anim_iri.material_class = iridescent_class;
        let anim_iri_mat = spawn_material(&mut scene_db.world, anim_iri);

        // Animated brush direction metal
        let mut aniso2 = make_mat([0.55, 0.55, 0.65, 1.0], 0.12, 0.9, [0.0; 3], 0.0);
        aniso2.material_class = MATERIAL_CLASS_ANISOTROPIC;
        let aniso2_mat = spawn_material(&mut scene_db.world, aniso2);

        // Opal: milky translucent body with play-of-colour from internal
        // 3D cell noise.  The opal template uses SSS for the translucent body
        // and a hash-based cell noise for the coloured patches.
        // class_params.x = patch_scale, .y = patch_strength, .z = view_shift
        let opal_component: MaterialComponent = GpuMaterial {
            base_color: [0.88, 0.84, 0.78, 1.0],
            emissive: [0.0; 4],
            roughness_metallic: [0.06, 0.0, 1.45, 0.0],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: 0,
            material_class: opal_class,
            class_params: [3.0, 1.0, 0.4, 0.0],
        }
        .into();
        let opal_mat = spawn_material(&mut scene_db.world, opal_component);

        // ── Glass material ────────────────────────────────────────────────────

        let glass_component: MaterialComponent = GpuMaterial {
            base_color: [0.85, 0.90, 0.95, 0.70], // slightly blue-tinted glass
            emissive: [0.0; 4],
            roughness_metallic: [0.015, 0.0, 1.5, 0.0],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: helio_mats::FLAG_TRANSPARENT_ONLY,
            material_class: glass_class,
            class_params: [0.0; 4],
        }
        .into();
        let glass_mat = spawn_material(&mut scene_db.world, glass_component);

        // ── Water material ────────────────────────────────────────────────────

        let water_component: MaterialComponent = GpuMaterial {
            base_color: [0.02, 0.1, 0.15, 0.85],
            emissive: [0.0; 4],
            roughness_metallic: [0.02, 0.0, 1.33, 0.0],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow: 0,
            flags: 0,
            material_class: water_class,
            class_params: [0.0; 4],
        }
        .into();
        let water_mat = spawn_material(&mut scene_db.world, water_component);

        // ── Meshes ───────────────────────────────────────────────────────────

        let sphere_mesh_id = spawn_mesh(&mut scene_db.world, sphere_mesh([0.0; 3], 1.0));

        let plane_mesh_id = spawn_mesh(&mut scene_db.world, plane_mesh([0.0; 3], 16.0));

        // ── Scene objects ────────────────────────────────────────────────────

        let s = 2.5;
        let yp = 0.0;
        let front_z = 0.0;
        let back_z = -5.0;

        // Dark ground plane
        let plane_mat = spawn_material(
            &mut scene_db.world,
            make_mat([0.03, 0.03, 0.035, 1.0], 0.95, 0.0, [0.0; 3], 0.0),
        );
        let _ = spawn_object(
            &mut scene_db.world,
            plane_mesh_id,
            plane_mat,
            Mat4::from_translation(Vec3::new(0.0, -1.5, -2.5)),
            16.0,
        );

        // ── Front row ──
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            gold_mat,
            Mat4::from_translation(Vec3::new(-s * 1.5, yp, front_z)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            plastic_mat,
            Mat4::from_translation(Vec3::new(-s * 0.5, yp, front_z)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            coat_mat,
            Mat4::from_translation(Vec3::new(s * 0.5, yp, front_z)),
            1.0,
        );

        // ── Back row ──
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            crystal_mat,
            Mat4::from_translation(Vec3::new(-s * 2.0, yp, back_z)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            aniso_mat,
            Mat4::from_translation(Vec3::new(-s * 1.0, yp, back_z)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            skin_mat,
            Mat4::from_translation(Vec3::new(s * 0.0, yp, back_z)),
            1.0,
        );
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            aniso2_mat,
            Mat4::from_translation(Vec3::new(s * 1.0, yp, back_z)),
            1.0,
        );
        // Glass: transparent sphere with Fresnel reflections
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            glass_mat,
            Mat4::from_translation(Vec3::new(-s * 2.5, yp + 0.5, front_z + 1.0)),
            0.8,
        );

        // Opal: milky body + iridescent play-of-colour
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            opal_mat,
            Mat4::from_translation(Vec3::new(s * 2.0, yp, back_z)),
            1.0,
        );

        // Water: animated wave surface
        let _ = spawn_object(
            &mut scene_db.world,
            sphere_mesh_id,
            water_mat,
            Mat4::from_translation(Vec3::new(s * 3.5, yp - 0.3, back_z - 0.5)),
            1.2,
        );

        // ── Lights ───────────────────────────────────────────────────────────

        // Key light: bright, close, sharp — makes specular highlights POP
        let key_id = spawn_light(
            &mut scene_db.world,
            point_light([4.0, 6.0, 3.0], [2.0, 1.8, 1.5], 18.0, 25.0),
        );

        // Second key from opposite side for backlighting (reveals SSS transmission)
        spawn_light(
            &mut scene_db.world,
            point_light([-3.0, 4.0, -4.0], [0.6, 0.8, 1.2], 12.0, 20.0),
        );

        // Fill
        spawn_light(
            &mut scene_db.world,
            directional_light([-0.3, -0.5, -0.4], [0.3, 0.35, 0.4], 0.5),
        );

        // Sun (orbits)
        let sun_id = spawn_light(
            &mut scene_db.world,
            directional_light([0.4, -0.8, 0.3], [1.0, 0.9, 0.75], 2.0),
        );

        // ── Sky: nearly black — reflections visible only from lights ──────────

        renderer.set_feature_live(helio::AmbientLight {
            color: [0.01, 0.01, 0.02],
            intensity: 0.03,
        });

        // ── Legend ───────────────────────────────────────────────────────────

        log::info!("");
        log::info!("═══ Helio Radiant Material Demo ═══");
        log::info!("  ── Front row ──────────────────────────");
        log::info!("  [-6.25] Glass             (Tier 3, glass/Fresnel shader)");
        log::info!("  [-3.75] Gold metallic     (Tier 1, metallic flags)");
        log::info!("  [-1.25] Red plastic       (Tier 1, matte dielectric)");
        log::info!("  [ 1.25] Clear coat        (Tier 2, clear_coat template)");
        log::info!("  [ 3.75] (removed -- was the Tier 2 graph-snippet showcase;");
        log::info!("           see this file's comment on why it has no SceneDB replacement)");
        log::info!("  ── Back row ───────────────────────────");
        log::info!("  [-5.00] Crystal/gemstone  (Tier 2, SSS, animated tint)");
        log::info!("  [-2.50] Brushed metal     (Tier 2, anisotropic GGX)");
        log::info!("  [ 0.00] Skin              (Tier 2, skin template)");
        log::info!("  [ 2.50] Aniso spinning    (Tier 2, aniso, anim direction)");
        log::info!("  [ 5.00] Opal              (Tier 3, custom opal shader)");
        log::info!("  [ 8.75] Water             (Tier 3, animated water shader)");
        log::info!("");

        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format,
            alpha_mode,
            renderer,
            scene_db,
            last_frame: Instant::now(),
            cam_pos: Vec3::new(0.0, 1.5, 6.0),
            yaw: 0.0,
            pitch: -0.1,
            velocity: Vec3::ZERO,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            animated_iri_id: anim_iri_mat,
            crystal_mat_id: crystal_mat,
            aniso_mat_id: aniso2_mat,
            sun_light_id: sun_id,
            key_light_id: key_id,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    state.surface.configure(
                        &state.device,
                        &wgpu::SurfaceConfiguration {
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                            format: state.surface_format,
                            width: new_size.width,
                            height: new_size.height,
                            present_mode: wgpu::PresentMode::Fifo,
                            alpha_mode: state.alpha_mode,
                            view_formats: vec![],
                            desired_maximum_frame_latency: 2,
                            color_space: wgpu::SurfaceColorSpace::Auto,
                        },
                    );
                    state
                        .renderer
                        .set_render_size(new_size.width, new_size.height);
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
                if state.cursor_grabbed {
                    state.cursor_grabbed = false;
                    state.window.set_cursor_visible(true);
                    let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(code),
                        ..
                    },
                ..
            } => {
                let _ = state.keys.insert(code);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Released,
                        physical_key: PhysicalKey::Code(code),
                        ..
                    },
                ..
            } => {
                state.keys.remove(&code);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if !state.cursor_grabbed => {
                let ok = state
                    .window
                    .set_cursor_grab(CursorGrabMode::Confined)
                    .or_else(|_| state.window.set_cursor_grab(CursorGrabMode::Locked))
                    .is_ok();
                if ok {
                    state.cursor_grabbed = true;
                    state.window.set_cursor_visible(false);
                }
            }
            WindowEvent::CursorMoved { position: pos, .. } if state.cursor_grabbed => {
                let center = (
                    state.window.inner_size().width as f64 / 2.0,
                    state.window.inner_size().height as f64 / 2.0,
                );
                state.mouse_delta.0 += (pos.x - center.0) as f32;
                state.mouse_delta.1 += (pos.y - center.1) as f32;
                let _ = state
                    .window
                    .set_cursor_position(PhysicalPosition::new(center.0 as i32, center.1 as i32));
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now.duration_since(state.last_frame).as_secs_f32().min(0.05);
                state.last_frame = now;
                state.update(dt);
                let size = state.window.inner_size();
                let camera = state.camera(size.width, size.height);

                // Orbit the key light for moving specular highlights
                let t = now.duration_since(state.last_frame).as_secs_f32();
                let key_x = (t * 0.15).cos() * 5.0;
                let key_z = (t * 0.15).sin() * 5.0 + 2.0;
                update_light(
                    &mut state.scene_db.world,
                    state.key_light_id,
                    point_light([key_x, 5.0, key_z], [2.0, 1.8, 1.5], 18.0, 25.0),
                );

                // Animate iridescent
                if let Some(mut m) = state
                    .scene_db
                    .world
                    .get_mut::<MaterialComponent>(state.animated_iri_id)
                {
                    m.class_params = [
                        3.0 + (t * 0.3).sin() * 2.0,
                        0.5 + (t * 0.5).sin() * 0.5,
                        0.0,
                        0.0,
                    ];
                }

                // Animate crystal: cycle internal colour
                let crystal_tint = [
                    0.2 + (t * 0.7).cos() * 0.3,
                    0.2 + (t * 0.9 + 2.0).cos() * 0.3,
                    0.2 + (t * 1.1 + 4.0).cos() * 0.35,
                ];
                if let Some(mut m) = state
                    .scene_db
                    .world
                    .get_mut::<MaterialComponent>(state.crystal_mat_id)
                {
                    m.class_params = [
                        crystal_tint[0],
                        crystal_tint[1],
                        crystal_tint[2],
                        3.0 + (t * 0.5).sin() * 1.5,
                    ];
                }

                // Animate anisotropic: rotate brush direction
                if let Some(mut m) = state
                    .scene_db
                    .world
                    .get_mut::<MaterialComponent>(state.aniso_mat_id)
                {
                    m.class_params = [0.9, t * 0.3, 0.0, 0.0];
                }

                // Upload this frame's SceneDB writes to the GPU mirror; without it they
                // are never visible to the renderer (Helio#266).
                v3_demo_common::flush_scene_db(&state.scene_db, state.renderer.queue());
                if let Err(e) = state.renderer.render_window_frame(&camera, &state.surface) {
                    log::error!("render error: {:?}", e);
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let Some(state) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if state.cursor_grabbed {
                state.mouse_delta.0 += dx as f32;
                state.mouse_delta.1 += dy as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl AppState {
    fn update(&mut self, dt: f32) {
        let (dx, dy) = self.mouse_delta;
        self.mouse_delta = (0.0, 0.0);
        self.yaw -= dx * LOOK_SENS;
        self.pitch = (self.pitch - dy * LOOK_SENS).clamp(-1.5, 1.5);
        let orientation = Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0);
        let forward = orientation * -Vec3::Z;
        let right = orientation * Vec3::X;
        let mut accel = Vec3::ZERO;
        if self.keys.contains(&KeyCode::KeyW) {
            accel += forward;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            accel -= forward;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            accel -= right;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            accel += right;
        }
        if self.keys.contains(&KeyCode::Space) {
            accel += Vec3::Y;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            accel -= Vec3::Y;
        }
        self.velocity += accel * FLY_SPEED * dt;
        self.velocity /= 1.0 + DRAG * dt;
        self.cam_pos += self.velocity * dt;
    }

    fn camera(&self, width: u32, height: u32) -> Camera {
        let orientation = Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0);
        Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + orientation * -Vec3::Z,
            orientation * Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            width as f32 / height.max(1) as f32,
            0.01,
            2000.0,
        )
    }
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let mut app = App { state: None };
    event_loop.run_app(&mut app).unwrap();
}
