//! A live preview of the render graph being edited: a full Helio renderer, in a
//! panel above the Properties panel, drawing a small built-in test scene
//! through the graph's pipeline.
//!
//! It shares the editor's wgpu device (the window's surface), so nothing is
//! duplicated on the GPU. The scene is private to the preview (its own SceneDB),
//! so it works with no level open and never touches one. The graph is applied
//! when the author presses Compile or saves, not on every edit: a rebuild
//! compiles shaders, and a half-wired graph never reaches the GPU.
//!
//! This is deliberately small. The level editor's `HelioViewport` does more
//! (input, gizmos, frame pacing) but is tied to the level editor's state; making
//! it reusable is tracked separately.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blueprint_graph::BlueprintAsset;
use engine_backend::scene::{new_scene, resolve_level_assets, RuntimeLevel};
use engine_backend::services::gpu_renderer::{GpuRenderer, GpuRendererBuilder};
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
    surface: Option<WgpuSurfaceHandle>,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    last_frame: u64,
    pump_started: bool,
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
                let _ = std::fs::remove_file(path);
            }
            Err(error) => tracing::warn!("Preview scene could not be written: {error}"),
        }
        let engine = GpuRendererBuilder::new(1, 1).scene_db(scene).build();
        let preview = Self {
            engine: Arc::new(Mutex::new(engine)),
            surface: None,
            stop: Arc::new(AtomicBool::new(false)),
            frames: Arc::new(AtomicU64::new(0)),
            last_frame: 0,
            pump_started: false,
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

    /// Runs the graph through the preview renderer, if it is sound.
    pub fn apply_graph(&mut self, document: &BlueprintAsset, cx: &mut Context<Self>) {
        let asset = model::buildable_asset(document, model::registry());
        let problems = model::problems(&asset, model::registry());
        if !problems.is_empty() {
            self.status = Some((true, problems.join("; ")));
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

    fn create_surface(&mut self, window: &mut Window) {
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
                self.start_render_thread(surface.clone());
                self.surface = Some(surface);
            }
            None => tracing::warn!("Render graph preview: no surface"),
        }
    }

    fn start_render_thread(&self, surface: WgpuSurfaceHandle) {
        engine_backend::services::device_loss::DeviceLoss::adopt(
            surface.device(),
            surface.device_lost_flag(),
        );
        let engine = self.engine.clone();
        let stop = self.stop.clone();
        let frames = self.frames.clone();
        let spawned = std::thread::Builder::new()
            .name("Render graph preview".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while !stop.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(16));
                        let (device, queue, format) =
                            (surface.device(), surface.queue(), surface.format());
                        let (width, height) = surface.size();
                        if width == 0 || height == 0 {
                            continue;
                        }
                        let mut engine = match engine.lock() {
                            Ok(engine) => engine,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        // Building the graph compiles pipelines: before the submit
                        // guard, so the window can still resize meanwhile.
                        if !engine.prepare_for_render(device, queue, width, height, format) {
                            continue;
                        }
                        let _guard = surface.submit_guard();
                        let Some((view, (width, height))) = surface.back_view_with_size() else {
                            continue;
                        };
                        let submission = engine
                            .render_frame_to_surface(device, queue, &view, width, height, format);
                        drop(view);
                        drop(engine);
                        if let Some(index) = submission {
                            surface.present_synced_silent(index);
                            frames.fetch_add(1, Ordering::Release);
                            surface.request_frame();
                        }
                    }
                }));
                if outcome.is_err() {
                    tracing::error!("Render graph preview thread panicked");
                }
            });
        if let Err(error) = spawned {
            tracing::error!("Render graph preview thread did not start: {error}");
        }
    }

    /// Repaints when the render thread publishes a frame.
    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let alive = this.update_in(cx, |this, _window, cx| {
                let published = this.frames.load(Ordering::Acquire);
                if published != this.last_frame {
                    this.last_frame = published;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
    }
}

fn write_test_scene() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "pulsar_render_preview_{}.level.json",
        std::process::id()
    ));
    std::fs::write(&path, TEST_SCENE)?;
    Ok(path)
}

impl Drop for PipelinePreview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
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
}

impl Render for PipelinePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.surface.is_none() {
            self.create_surface(window);
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

thread_local! {
    /// The open previews by the file their editor edits. The editor calls the
    /// profile on the UI thread, which is where these are used.
    static PREVIEWS: std::cell::RefCell<Vec<(Option<PathBuf>, WeakEntity<PipelinePreview>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Remembers `preview` as the preview of the editor of `file`.
pub fn register(file: Option<PathBuf>, preview: WeakEntity<PipelinePreview>) {
    PREVIEWS.with(|previews| previews.borrow_mut().push((file, preview)));
}

/// The previews of the editor of `file`.
pub fn previews_for(file: Option<&std::path::Path>) -> Vec<WeakEntity<PipelinePreview>> {
    PREVIEWS.with(|previews| {
        let mut previews = previews.borrow_mut();
        // Closed editors leave dead entries behind.
        previews.retain(|(_, preview)| preview.upgrade().is_some());
        let matching: Vec<_> = previews
            .iter()
            .filter(|(path, _)| path.as_deref() == file)
            .map(|(_, preview)| preview.clone())
            .collect();
        // The path can be unknown while the workspace is built (a file still
        // loading); then every open preview is as good as any.
        if matching.is_empty() {
            previews
                .iter()
                .map(|(_, preview)| preview.clone())
                .collect()
        } else {
            matching
        }
    })
}
