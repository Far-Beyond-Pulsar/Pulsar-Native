//! Dockable material preview panel

use gpui::*;
use ui::dock::{Panel, PanelEvent};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, IconName,
};

use super::mesh::{self, PreviewMeshData};
use super::renderer::PreviewRenderer;
use crate::editor::panel::MeshType;

pub struct MaterialPreviewPanel {
    editor: WeakEntity<crate::editor::panel::ShaderEditorPanel>,
    focus_handle: FocusHandle,
    renderer: PreviewRenderer,
    pub current_mesh: MeshType,
    pub auto_rotate: bool,
    pub auto_rotate_speed: f32,
    surface_handle: Option<gpui::WgpuSurfaceHandle>,
    needs_rebuild: bool,
    last_shader_source: Option<String>,
    /// The in-flight background shader build. Replacing it drops (cancels)
    /// a superseded one, so rapid edits only ever apply the newest shader.
    shader_build: Option<Task<()>>,
    /// Right mouse button is held over the viewport — drag orbits the
    /// camera, and scroll zooms in/out while this is true.
    orbiting: bool,
    last_drag_pos: Option<Point<Pixels>>,
    subscriptions: Vec<Subscription>,
    /// When the last frame was rendered; paces the animation and derives `dt`.
    last_frame: Option<std::time::Instant>,
    surface_animation: plugin_editor_api::surface_animation::SurfaceAnimation,
}

/// Pace GPU surface updates independently of the editor layout.
const PREVIEW_FRAME: std::time::Duration = std::time::Duration::from_millis(33);

impl MaterialPreviewPanel {
    pub fn new(
        editor: WeakEntity<crate::editor::panel::ShaderEditorPanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            editor,
            focus_handle: cx.focus_handle(),
            renderer: PreviewRenderer::new(),
            current_mesh: MeshType::Sphere,
            auto_rotate: true,
            auto_rotate_speed: 0.5,
            surface_handle: None,
            needs_rebuild: true,
            last_shader_source: None,
            shader_build: None,
            orbiting: false,
            last_drag_pos: None,
            subscriptions: Vec::new(),
            last_frame: None,
            surface_animation: Default::default(),
        }
    }

    fn on_orbit_mouse_down(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                panel.orbiting = true;
                panel.last_drag_pos = Some(event.position);
                cx.notify();
            });
        }
    }

    fn on_orbit_mouse_move(
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseMoveEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                if !panel.orbiting {
                    return;
                }
                let Some(last) = panel.last_drag_pos else {
                    return;
                };

                let dx = (event.position.x - last.x).to_f64() as f32;
                let dy = (event.position.y - last.y).to_f64() as f32;
                panel.last_drag_pos = Some(event.position);

                const ORBIT_SENSITIVITY: f32 = 0.01;
                if let Some(editor) = panel.editor.upgrade() {
                    editor.update(cx, |ed, _cx| {
                        let (yaw, pitch) = ed.preview_rotation;
                        let new_yaw = yaw + dx * ORBIT_SENSITIVITY;
                        let new_pitch = (pitch + dy * ORBIT_SENSITIVITY).clamp(-1.5, 1.5);
                        ed.preview_rotation = (new_yaw, new_pitch);
                    });
                }
                cx.notify();
            });
        }
    }

    fn on_orbit_mouse_up(cx: &mut Context<Self>) -> impl Fn(&MouseUpEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |_event, _window, cx| {
            entity.update(cx, |panel, cx| {
                panel.orbiting = false;
                panel.last_drag_pos = None;
                cx.notify();
            });
        }
    }

    fn on_orbit_scroll(
        cx: &mut Context<Self>,
    ) -> impl Fn(&ScrollWheelEvent, &mut Window, &mut App) {
        let entity = cx.entity().clone();
        move |event, _window, cx| {
            entity.update(cx, |panel, cx| {
                if !panel.orbiting {
                    return;
                }

                let delta_y = match event.delta {
                    ScrollDelta::Pixels(p) => p.y.to_f64() as f32,
                    ScrollDelta::Lines(l) => l.y * 20.0,
                };

                const ZOOM_SENSITIVITY: f32 = 0.002;
                let factor = (1.0 - delta_y * ZOOM_SENSITIVITY).clamp(0.5, 1.5);
                let camera = &mut panel.renderer.camera;
                camera.distance = (camera.distance * factor).clamp(0.2, 100.0);
                cx.notify();
            });
        }
    }

    pub fn rebuild_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.needs_rebuild {
            return;
        }

        let size = window.bounds().size;
        let width = (size.width.to_f64() as u32).max(1);
        let height = (size.height.to_f64() as u32).max(1);

        // Until the window has a new device after a loss, a new surface is on
        // the lost one too; the window renders every view again once it has.
        let Some(surface) = window
            .create_wgpu_surface(width, height, wgpu::TextureFormat::Bgra8Unorm)
            .filter(|surface| !surface.is_device_lost())
        else {
            return;
        };

        // The renderer has no device/queue of its own until `initialize()`
        // runs below — pull them from the freshly created surface instead
        // of `self.renderer.device`/`queue` (which would always be `None`
        // here, causing this function to bail out on every call).
        let device = surface.device().clone();
        let queue = surface.queue().clone();

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };

        self.renderer.initialize(&device, &queue, &config);
        self.renderer.update_camera(width as f32 / height as f32);

        // Upload the preview mesh now that the renderer has a device/queue —
        // without this, `PreviewRenderer::render()` bails out before ever
        // clearing the surface, leaving it grey.
        let mesh_data = generate_mesh_data(self.current_mesh);
        self.renderer
            .camera
            .frame_bounding_radius(mesh_data.bounding_radius());
        self.renderer.update_mesh(
            &mesh_data.vertices,
            &mesh_data.indices,
            mesh_data.index_count,
        );

        self.surface_handle = Some(surface);
        self.needs_rebuild = false;
    }

    pub fn update_mesh(&mut self, mesh_type: MeshType) {
        self.current_mesh = mesh_type;
        let mesh_data = generate_mesh_data(mesh_type);
        self.renderer
            .camera
            .frame_bounding_radius(mesh_data.bounding_radius());
        self.renderer.update_mesh(
            &mesh_data.vertices,
            &mesh_data.indices,
            mesh_data.index_count,
        );
        self.needs_rebuild = true;
    }

    /// Compile `wgsl_source` into the preview pipeline without blocking the
    /// editor: shader compilation and texture decoding run on a worker, and
    /// the finished pipeline is swapped in while the previous one keeps
    /// drawing. A shader the GPU rejects leaves the old pipeline in place.
    pub fn update_shader(&mut self, wgsl_source: &str, cx: &mut Context<Self>) {
        if self.last_shader_source.as_deref() == Some(wgsl_source) {
            return;
        }
        // Not initialized yet: leave `last_shader_source` unset so the next
        // frame retries instead of dropping this shader.
        let Some(inputs) = self.renderer.shader_build_inputs() else {
            return;
        };
        self.last_shader_source = Some(wgsl_source.to_string());
        let source = wgsl_source.to_string();
        self.shader_build = Some(cx.spawn(async move |this, cx| {
            let build = cx
                .background_executor()
                .spawn(async move { super::renderer::build_shader(&inputs, &source) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let error = build.error.clone();
                this.renderer.apply_shader(build);
                // Surface the outcome in the Compiler Output panel; a later
                // successful build clears a stale error.
                if let Some(editor) = this.editor.upgrade() {
                    editor.update(cx, |editor, cx| {
                        editor.set_material_preview_error(error, cx)
                    });
                }
                cx.notify();
            });
        }));
    }

    fn render_surface(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        profiling::profile_scope!("shader preview: surface frame");
        if self.auto_rotate && !self.orbiting {
            if let Some(editor) = self.editor.upgrade() {
                let (yaw, pitch) = editor.read(cx).preview_rotation;
                let dt = self
                    .last_frame
                    .map_or(0.016, |t| t.elapsed().as_secs_f32().min(0.1));
                let new_yaw = yaw + self.auto_rotate_speed * dt;
                editor.update(cx, |panel, _cx| {
                    panel.preview_rotation = (new_yaw, pitch);
                });
            }
        }

        if let Some(editor) = self.editor.upgrade() {
            let (yaw, pitch) = editor.read(cx).preview_rotation;
            self.renderer.camera.yaw = yaw;
            self.renderer.camera.pitch = pitch;
        }

        let wgsl_to_compile: Option<String> = self
            .editor
            .upgrade()
            .and_then(|editor| editor.read(cx).last_compiled_wgsl.clone());

        // Nothing compiles until the user presses Compile (F7): opening or
        // editing a graph must never start shader compilation on its own.
        if let Some(ref wgsl) = wgsl_to_compile {
            if self.renderer.device.is_some() && self.renderer.queue.is_some() {
                self.update_shader(wgsl, cx);
            }
        }

        if let Some(surface) = &self.surface_handle {
            // The wgpu surface resizes itself to match its layout bounds
            // independently of `rebuild_surface` — keep the camera's aspect
            // ratio in sync every frame so the projection matches the
            // surface's actual size instead of stretching to whatever size
            // it was when the surface was first created.
            let (width, height) = surface.size();
            if height > 0 {
                self.renderer.update_camera(width as f32 / height as f32);
            }

            if let Some(view) = surface.back_buffer_view() {
                self.renderer.render(&view);
                surface.swap_buffers();
            }
        }

        self.last_frame = Some(std::time::Instant::now());
    }
    fn render_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        profiling::profile_scope!("shader preview: render");
        // The GPU device was lost: the surface, the renderer's resources and
        // the compiled material are on it. Start over on the window's new
        // device, compiling the material again.
        if self
            .surface_handle
            .as_ref()
            .is_some_and(gpui::WgpuSurfaceHandle::is_device_lost)
        {
            tracing::warn!("Shader preview: GPU device lost; rebuilding on the new device");
            self.surface_handle = None;
            self.renderer = PreviewRenderer::new();
            self.shader_build = None;
            self.last_shader_source = None;
            self.needs_rebuild = true;
        }
        if self.needs_rebuild {
            self.rebuild_surface(window, cx);
        }

        // The rendered texture only appears once `wgpu_surface()` is present
        // in the element tree — the surface element handles resizing itself
        // to match its layout bounds, so the initial size doesn't matter.
        //
        // Sized via normal flow (`flex_1` + `min_h`) rather than absolute
        // positioning: an absolutely-positioned child doesn't contribute to
        // its parent's intrinsic size, so if the parent's height ever ends
        // up "auto" the surface collapses to zero height and the dock shows
        // its own background instead of the rendered frame.
        let gpu_display: AnyElement = if let Some(surface) = &self.surface_handle {
            wgpu_surface(surface.clone())
                .defer_resize_until_mouse_up(true)
                .size_full()
                .into_any_element()
        } else {
            div().size_full().bg(gpui::rgb(0x1a1a1a)).into_any_element()
        };

        // Shaders only compile when the user presses Compile, so say so
        // instead of showing an unexplained empty viewport.
        let no_shader_hint = self
            .editor
            .upgrade()
            .is_some_and(|editor| editor.read(cx).last_compiled_wgsl.is_none())
            .then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Press Compile (F7) to preview this material")
            });

        div()
            .size_full()
            .min_h(px(200.))
            .flex_1()
            .overflow_hidden()
            .bg(gpui::rgb(0x1a1a1a))
            .on_mouse_down(MouseButton::Right, Self::on_orbit_mouse_down(cx))
            .on_mouse_move(Self::on_orbit_mouse_move(cx))
            .on_mouse_up(MouseButton::Right, Self::on_orbit_mouse_up(cx))
            .on_mouse_up_out(MouseButton::Right, Self::on_orbit_mouse_up(cx))
            .on_scroll_wheel(Self::on_orbit_scroll(cx))
            .relative()
            .child(gpu_display)
            .children(no_shader_hint)
            .child(
                plugin_editor_api::surface_animation::surface_animation(
                    &cx.entity(),
                    &mut self.surface_animation,
                    PREVIEW_FRAME,
                    |panel, _, window, cx| {
                        panel.render_surface(window, cx);
                        true // Material shaders and the sky can depend on time even without rotation.
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .into_any_element()
    }
}

fn generate_mesh_data(mesh_type: MeshType) -> PreviewMeshData {
    match mesh_type {
        MeshType::Sphere => mesh::generate_sphere(1.0, 32, 24),
        MeshType::Quad => mesh::generate_quad(),
        MeshType::Cube => mesh::generate_cube(),
        MeshType::Cinderblock => mesh::generate_cinderblock(),
    }
}

impl EventEmitter<PanelEvent> for MaterialPreviewPanel {}

impl Render for MaterialPreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h(px(200.))
            .child(self.render_preview(window, cx))
    }
}

impl Focusable for MaterialPreviewPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MaterialPreviewPanel {
    fn panel_name(&self) -> &'static str {
        "material-preview"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Material Preview".into_any_element()
    }
}
