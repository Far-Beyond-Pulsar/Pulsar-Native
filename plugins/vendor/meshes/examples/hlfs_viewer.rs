//! Small shared interactive host for architectural RT scenes.
//! Post-process volumes left Helio with their plugin (Pulsar
//! `plugins/vendor/post`); the viewer no longer outlines them.
use crate::v3_demo_common::*;
use glam::Vec3;
use helio::{
    Camera, HelioGraphicsConfig, HelioGraphicsContext, Renderer, RendererBuilder, RendererConfig,
};
use pulsar_scenedb::{Entity, SceneDb, World};
use std::{collections::HashSet, sync::Arc, time::Instant};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

pub struct Scene {
    pub name: &'static str,
    pub populate: Box<dyn FnMut(&mut World) -> (Vec<Entity>, Vec<Entity>)>,
    pub update: Option<Box<dyn FnMut(&mut World, f32)>>,
    pub key: Option<Box<dyn FnMut(&mut World, KeyCode)>>,
    pub camera: fn(f32, f32) -> Camera,
    pub orbit_target: Vec3,
}
pub fn run(scene: Scene) {
    let event_loop = EventLoop::new().expect("event loop");
    event_loop
        .run_app(&mut App { scene, state: None })
        .expect("viewer");
}
struct App {
    scene: Scene,
    state: Option<State>,
}
struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    renderer: Renderer,
    scene_db: SceneDb,
    acceleration: Option<helio_mesh_host::ray_tracing::SceneDbRayTracing>,
    keys: HashSet<KeyCode>,
    last: Instant,
    angle: f32,
    distance: f32,
    height: f32,
    elapsed: f32,
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window=Arc::new(event_loop.create_window(Window::default_attributes()
            .with_title(format!("Helio - {} | A/D orbit, W/S zoom, Q/E height | G quality, L lens, O occluder, M motion, P profile",self.scene.name))
            .with_inner_size(winit::dpi::LogicalSize::new(1280u32,720u32))).expect("window"));
        let size = window.inner_size();
        let mut graphics = pollster::block_on(HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            HelioGraphicsConfig {
                vr_support: true,
                application_name: format!("Helio {}", self.scene.name),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let config = graphics.surface_config.clone();
        let format = config.format;
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        (self.scene.populate)(&mut scene_db.world);
        let rt = std::env::var_os("HLFS_RT").is_some();
        let presampled = std::env::var_os("HLFS_PRESAMPLED").is_some();
        assert!(!presampled || rt, "HLFS_PRESAMPLED requires HLFS_RT");
        let hlfs = if presampled {
            helio_pass_hlfs::HlfsConfig::ray_traced_presampled()
        } else {
            helio_pass_hlfs::HlfsConfig {
                mode: if rt {
                    helio_pass_hlfs::HlfsMode::RayTraced
                } else {
                    helio_pass_hlfs::HlfsMode::ScreenSpace
                },
                ..Default::default()
            }
        };
        let mut renderer = RendererBuilder::new(
            RendererConfig::new(config.width, config.height, format)
                .with_ssr(std::env::var_os("HLFS_SSR").is_some()),
            scene_db_handle(&scene_db),
        )
        .with_editor_mode(true)
        .with_pass_build_context(Box::new(move |ctx| {
            let device = ctx.device.clone();
            let mut graph = helio_mesh_host::presets::hlfs_fxaa(ctx);
            graph
                .find_pass_mut::<helio_pass_hlfs::HlfsPass>()
                .expect("HLFS")
                .set_config(&device, hlfs);
            graph
        }))
        .build_with_graphics_context(&mut graphics)
        .expect("Helio renderer");
        let surface = graphics.surface;
        renderer.set_feature_live(helio::AmbientLight {
            color: [0.05, 0.05, 0.08],
            intensity: 1.0,
        });
        let acceleration = if rt {
            crate::hlfs_capture::enable_ray_shadows(&mut scene_db.world);
            Some(helio_mesh_host::ray_tracing::SceneDbRayTracing::new(
                device.clone(),
                queue.clone(),
            ))
        } else {
            None
        };
        let offset = (self.scene.camera)(0.0, config.width as f32 / config.height as f32)
            .eye
            .as_vec3()
            - self.scene.orbit_target;
        self.state = Some(State {
            window,
            surface,
            config,
            device,
            queue,
            renderer,
            scene_db,
            acceleration,
            keys: HashSet::new(),
            last: Instant::now(),
            angle: offset.x.atan2(offset.z),
            distance: offset.x.hypot(offset.z),
            height: offset.y,
            elapsed: 0.0,
        });
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(s) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => s.keys.clear(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if key == KeyCode::Escape {
                        event_loop.exit();
                    }
                    if event.state == ElementState::Pressed {
                        if !event.repeat {
                            if let Some(handler) = &mut self.scene.key {
                                handler(&mut s.scene_db.world, key);
                            }
                        }
                        s.keys.insert(key);
                    } else {
                        s.keys.remove(&key);
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    s.config.width = size.width;
                    s.config.height = size.height;
                    s.surface.configure(&s.device, &s.config);
                    s.renderer.set_render_size(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let dt = s.last.elapsed().as_secs_f32().min(0.05);
                s.last = Instant::now();
                let axis = |plus, minus| {
                    f32::from(s.keys.contains(&plus)) - f32::from(s.keys.contains(&minus))
                };
                s.angle += axis(KeyCode::KeyD, KeyCode::KeyA) * dt * 0.6;
                s.distance =
                    (s.distance + axis(KeyCode::KeyS, KeyCode::KeyW) * dt * 30.0).clamp(3.0, 250.0);
                s.height = (s.height + axis(KeyCode::KeyE, KeyCode::KeyQ) * dt * 20.0)
                    .clamp(-self.scene.orbit_target.y + 0.5, 150.0);
                s.elapsed += dt;
                if let Some(update) = &mut self.scene.update {
                    update(&mut s.scene_db.world, s.elapsed);
                }
                let eye = self.scene.orbit_target
                    + Vec3::new(
                        s.angle.sin() * s.distance,
                        s.height,
                        s.angle.cos() * s.distance,
                    );
                let aspect = s.config.width as f32 / s.config.height as f32;
                let mut camera = Camera::perspective_look_at(
                    eye,
                    self.scene.orbit_target,
                    Vec3::Y,
                    0.85,
                    aspect,
                    0.1,
                    500.0,
                );
                camera.view_id = (self.scene.camera)(0.0, aspect).view_id;
                if s.window.inner_size().width == 0 || s.window.inner_size().height == 0 {
                    return;
                }
                flush_scene_db(&s.scene_db, &s.queue);
                if let Some(acceleration) = &mut s.acceleration {
                    // Refuse geometry the renderer draws but the projection cannot trace.
                    acceleration.set_untraced_geometry(s.renderer.untraced_geometry());
                    acceleration
                        .prepare(&s.scene_db.world)
                        .expect("scene acceleration");
                    acceleration.publish(&mut s.renderer);
                }
                s.renderer
                    .render_window_frame(&camera, &s.surface)
                    .expect("scene frame");
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}
