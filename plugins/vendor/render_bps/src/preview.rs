//! A live preview of the render graph being edited: a full Helio renderer, in a
//! panel above the Properties panel, drawing a small built-in test scene
//! through the graph's pipeline.
//!
//! It is an engine render runtime (`engine_backend::services::render_runtimes`)
//! drawing into the panel's surface on the editor's wgpu device, so nothing is
//! duplicated on the GPU and no thread is the preview's own; it draws while the
//! panel is shown and is listed in the Tasks window's Renderers view. The scene is private to the preview (its own SceneDB),
//! so it works with no level open and never touches one. The graph is applied
//! when the author presses Compile or saves, not on every edit: a rebuild
//! compiles shaders, and a half-wired graph never reaches the GPU.
//!
//! The preview owns its test scene, camera controls, and status UI. The surface
//! output and frame pump are shared with the level editor through
//! `ui_helio_viewport`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};

use engine_backend::scene::{new_scene, resolve_level_assets, RuntimeLevel};
use engine_backend::services::gpu_renderer::{GpuRenderer, GpuRendererBuilder};
use engine_backend::services::render_runtimes::{
    self, RuntimeDescription, RuntimeHandle, UpdateMode,
};
use engine_backend::subsystems::render::helio_renderer::pipelines;
use engine_backend::subsystems::render::EditorCameraState;
use gpui::*;
use ui::dock::{Panel, PanelEvent};
use ui::ActiveTheme as _;

use crate::model;

/// The preview scene, in the level file format: a ground plane, three shapes,
/// a sun and a point light.
const TEST_SCENE: &str = r#"{
  "version": "2.1",
  "objects": [
    { "id": "ground", "name": "Ground", "object_type": { "Mesh": "Plane" },
      "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [14.0, 1.0, 14.0] },
      "parent": null, "visible": true, "locked": false, "props": {} },
    { "id": "sphere", "name": "Sphere", "object_type": { "Mesh": "Sphere" },
      "transform": { "position": [-1.6, 0.5, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
      "parent": null, "visible": true, "locked": false, "props": {} },
    { "id": "cube", "name": "Cube", "object_type": { "Mesh": "Cube" },
      "transform": { "position": [1.6, 0.5, 0.0], "rotation": [0.0, 30.0, 0.0], "scale": [1.0, 1.0, 1.0] },
      "parent": null, "visible": true, "locked": false, "props": {} },
    { "id": "cylinder", "name": "Cylinder", "object_type": { "Mesh": "Cylinder" },
      "transform": { "position": [0.0, 0.5, -1.8], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
      "parent": null, "visible": true, "locked": false, "props": {} },
    { "id": "sun", "name": "Sun", "object_type": { "Light": "Directional" },
      "transform": { "position": [0.0, 6.0, 0.0], "rotation": [-50.0, 30.0, 0.0], "scale": [1.0, 1.0, 1.0] },
      "parent": null, "visible": true, "locked": false,
      "props": { "color_r": 1.0, "color_g": 0.96, "color_b": 0.9, "intensity": 3.0, "range": 50.0 } },
    { "id": "lamp", "name": "Lamp", "object_type": { "Light": "Point" },
      "transform": { "position": [0.0, 2.6, 2.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
      "parent": null, "visible": true, "locked": false,
      "props": { "color_r": 1.0, "color_g": 0.7, "color_b": 0.4, "intensity": 40.0, "range": 12.0 } }
  ],
  "components": {},
  "metadata": {},
  "editor": {}
}"#;

const FOCUS: [f64; 3] = [0.0, 0.8, 0.0];

/// The preview panel.
pub struct PipelinePreview {
    engine: Arc<Mutex<GpuRenderer>>,
    /// The engine runtime drawing `engine` into `surface`.
    runtime: RuntimeHandle,
    surface: Option<WgpuSurfaceHandle>,
    frames: Arc<AtomicU64>,
    display_refresh_millihertz: Arc<AtomicU32>,
    frame_pump: ui_helio_viewport::PublishedFramePump,
    yaw: f32,
    pitch: f32,
    distance: f32,
    drag_from: Option<Point<Pixels>>,
    /// The pipeline the renderer currently runs, so the next one replaces it.
    pipeline_id: Option<String>,
    /// What the last apply did: `true` is an error.
    status: Option<(bool, String)>,
    focus_handle: FocusHandle,
}

impl PipelinePreview {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let scene = Arc::new(parking_lot::RwLock::new(new_scene()));
        match write_test_scene() {
            Ok(path) => {
                let mut store = scene.write();
                match RuntimeLevel::load_into(&path, &mut store.world) {
                    Ok(_) => resolve_level_assets(&mut store.world),
                    Err(error) => tracing::warn!("Preview scene did not load: {error}"),
                }
                let _ = engine_fs::virtual_fs::delete_path(&path);
            }
            Err(error) => tracing::warn!("Preview scene could not be written: {error}"),
        }
        let engine = Arc::new(Mutex::new(
            GpuRendererBuilder::new(1, 1).scene_db(scene).build(),
        ));
        let runtime = render_runtimes::global().create(
            RuntimeDescription {
                label: "Render graph preview".into(),
                origin: pulsar_plugins::origin!(),
                scene_label: "preview test scene".into(),
                priority: 0,
                mode: UpdateMode::WhenVisible,
            },
            Arc::clone(&engine),
        );
        let preview = Self {
            engine,
            runtime,
            surface: None,
            frames: Arc::new(AtomicU64::new(0)),
            display_refresh_millihertz: Arc::new(AtomicU32::new(0)),
            frame_pump: ui_helio_viewport::PublishedFramePump::new(),
            yaw: 0.0,
            pitch: -0.25,
            distance: 6.0,
            drag_from: None,
            pipeline_id: None,
            status: None,
            focus_handle: cx.focus_handle(),
        };
        preview.apply_camera();
        preview
    }

    /// Runs the graph through the preview renderer, if it is sound. `asset` is the
    /// buildable projection of the editor's document.
    pub fn apply_asset(&mut self, asset: helio_graph_asset::GraphAsset, cx: &mut Context<Self>) {
        if let Some(blocked) = blocked(&asset, model::registry()) {
            self.status = Some((true, blocked));
            cx.notify();
            return;
        }
        let id = pipelines::register_preview_pipeline(asset, self.pipeline_id.as_deref());
        if let Ok(engine) = self.engine.lock() {
            if let Some(mailbox) = engine.editor_mailbox() {
                mailbox.set_render_pipeline(Some(id.clone()));
            }
        }
        self.pipeline_id = Some(id);
        self.status = Some((false, "Preview updated".to_string()));
        cx.notify();
    }

    fn apply_camera(&self) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // The renderer's `direction(yaw, pitch)`.
        let forward = [f64::from(sy * cp), f64::from(sp), f64::from(-cy * cp)];
        let distance = f64::from(self.distance);
        let position = [
            FOCUS[0] - forward[0] * distance,
            FOCUS[1] - forward[1] * distance,
            FOCUS[2] - forward[2] * distance,
        ];
        if let Ok(mut engine) = self.engine.lock() {
            engine.set_editor_camera_state(EditorCameraState {
                position,
                yaw: self.yaw,
                pitch: self.pitch,
            });
        }
    }

    fn create_surface(&mut self, window: &mut Window, refresh_hz: Option<f64>) {
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        match window.create_wgpu_surface_with_color_conversion(
            640,
            360,
            format,
            gpui::SurfaceColorConversion::LinearToSrgb,
        ) {
            // On a lost device the window rebuilds and renders this view again.
            Some(surface) if surface.is_device_lost() => {}
            Some(surface) => {
                if let Some(hz) = refresh_hz {
                    self.display_refresh_millihertz
                        .store((hz * 1000.0) as u32, std::sync::atomic::Ordering::Relaxed);
                }
                self.runtime
                    .set_output(Arc::new(ui_helio_viewport::SurfaceTarget::new(
                        surface.clone(),
                        self.frames.clone(),
                        self.display_refresh_millihertz.clone(),
                    )));
                self.surface = Some(surface);
            }
            None => tracing::warn!("Render graph preview: no surface"),
        }
    }

    /// Repaints when the runtime publishes a frame.
    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.frame_pump.start(self.frames.clone(), window, cx);
    }
}

/// Why a graph must not reach the preview renderer: a half-wired graph never
/// gets to the GPU. `None` when it is sound.
pub(crate) fn blocked(
    asset: &helio_graph_asset::GraphAsset,
    registry: &helio_graph_asset::NodeRegistry,
) -> Option<String> {
    let problems = model::problems(asset, registry);
    (!problems.is_empty()).then(|| problems.join("; "))
}

fn write_test_scene() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "pulsar_render_preview_{}.level.json",
        std::process::id()
    ));
    engine_fs::virtual_fs::write_file(&path, TEST_SCENE.as_bytes())
        .map_err(std::io::Error::other)?;
    Ok(path)
}

impl EventEmitter<PanelEvent> for PipelinePreview {}

impl Focusable for PipelinePreview {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PipelinePreview {
    fn panel_name(&self) -> &'static str {
        "render-graph-preview"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Preview".into_any_element()
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, _cx: &mut App) {
        self.runtime.set_visible(active);
    }
}

impl Render for PipelinePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.surface.is_none() {
            let refresh_hz = window
                .display(cx)
                .or_else(|| cx.primary_display())
                .and_then(|display| display.refresh_rate_millihertz())
                .filter(|millihertz| *millihertz > 0)
                .or_else(|| {
                    cx.displays()
                        .iter()
                        .filter_map(|display| display.refresh_rate_millihertz())
                        .max()
                        .filter(|millihertz| *millihertz > 0)
                })
                .map(|millihertz| millihertz as f64 / 1000.0);
            self.create_surface(window, refresh_hz);
        }
        if self.surface.is_some() {
            self.start_pump(window, cx);
        }

        let view = match self.surface.clone() {
            Some(surface) => wgpu_surface(surface)
                .absolute()
                .inset_0()
                .into_any_element(),
            None => div().into_any_element(),
        };
        let status = self.status.clone().map(|(is_error, text)| {
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .right_0()
                .px_2()
                .py_1()
                .text_xs()
                .bg(cx.theme().background.opacity(0.8))
                .text_color(if is_error {
                    cx.theme().danger
                } else {
                    cx.theme().muted_foreground
                })
                .child(text)
        });

        div()
            .id("render-graph-preview")
            .relative()
            .size_full()
            .min_h(px(180.0))
            .overflow_hidden()
            .bg(cx.theme().background)
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                    this.drag_from = Some(event.position);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, _cx| {
                    this.drag_from = None;
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, _cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    this.drag_from = None;
                    return;
                }
                if let Some(from) = this.drag_from {
                    let dx = f32::from(event.position.x - from.x);
                    let dy = f32::from(event.position.y - from.y);
                    this.yaw += dx * 0.01;
                    this.pitch = (this.pitch - dy * 0.01).clamp(-1.4, 1.4);
                    this.drag_from = Some(event.position);
                    this.apply_camera();
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, _cx| {
                let dy = f32::from(event.delta.pixel_delta(px(16.0)).y);
                this.distance = (this.distance - dy * 0.02).clamp(1.5, 30.0);
                this.apply_camera();
            }))
            .child(view)
            .children(status)
    }
}
