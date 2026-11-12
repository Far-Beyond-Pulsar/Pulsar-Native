//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::{prelude::FluentBuilder as _, *};
use plugin_editor_api::{AssetKind, AssetPayload};
use rust_i18n::t;
use ui::{notification::Notification, ActiveTheme as _, ContextModal};

use crate::commands::{execute_command, SceneCommand, TypedComponent};
use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;
use helio_component::asset_component::component_for_asset;
use pulsar_reflection::REGISTRY;

/// A GPUI component that shows a Helio renderer's frames from a
/// `WgpuSurfaceHandle`.
///
/// The renderer is an engine render runtime
/// (`engine_backend::services::render_runtimes`): the engine's scheduler draws
/// it into the surface (`ui_helio_viewport::SurfaceTarget`) while the view is
/// shown, so the UI thread is never blocked by GPU work and no viewport has a
/// render loop of its own. The view gives the runtime its surface, says when
/// it is shown, and repaints when a frame is published:
///
/// Presentation is deliberately split across the two threads:
/// - the runtime only publishes finished frames into the triple buffer
///   and bumps `frames_published`;
/// - the UI thread runs a frame pump that watches that counter and repaints
///   only when a new frame actually exists, so the compositor promotes
///   `ready` -> `display` exactly once per rendered frame.
///
/// The pump is deliberately *not* `Window::request_animation_frame()` in
/// `render()`. That notifies this view every single frame, and
/// `Window::mark_view_dirty` propagates dirt to every ancestor — the viewport
/// panel, the tab panel, the workspace and `LevelEditorPanel` itself — so the
/// entire level editor chrome was being rebuilt at event-loop rate regardless
/// of whether Helio had produced anything new.
pub struct HelioViewport {
    pub gpu_engine: Arc<Mutex<GpuRenderer>>,
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    surface: Option<WgpuSurfaceHandle>,
    focus_handle: FocusHandle,
    debug_replace_with_yellow: bool,
    /// The engine runtime that draws `gpu_engine` into `surface`.
    runtime: engine_backend::services::render_runtimes::RuntimeHandle,
    /// The loading overlay's token while the runtime builds its renderer.
    initial_load: Option<crate::state::EditorLoadingTask>,
    /// Whether the level editor's tab and this view's dock tab are in front.
    editor_tab_active: bool,
    dock_tab_active: bool,
    /// Incremented by the runtime each time it publishes a frame into the
    /// triple buffer. Read by the frame pump to decide whether a repaint is
    /// warranted.
    frames_published: Arc<AtomicU64>,
    /// Refresh rate of the display the window is on, in millihertz (0 = not
    /// known yet). Written on the UI thread and read by the runtime's output
    /// for its pace, since on Wayland it's often unknown at first.
    display_refresh_millihertz: Arc<AtomicU32>,
    /// When the display refresh rate was last sampled, while it's still unknown.
    last_display_refresh_sample: Instant,
    /// Value of `frames_published` at the last repaint this view requested.
    last_published_frame: u64,
    /// Loading state seen by the frame pump; changes repaint the shared
    /// initialization overlay even before Helio has produced its first frame.
    last_loading_snapshot: Option<crate::state::EditorLoadingTaskSnapshot>,
    last_screenshot_processing: bool,
    /// A repaint has been requested but `render()` has not run yet. Keeps the
    /// pump from stacking up notifies for a viewport that isn't being rendered.
    awaiting_render: bool,
    /// When this view last actually rendered. Drives the periodic real notify that
    /// keeps surface bounds tracking its element. Wall-clock, not a frame count:
    /// a count of published frames shrinks as the frame rate rises (150 frames
    /// was ~2.5 s at 60 fps but under 1 s at 165 fps, each one a ~9 ms rebuild).
    last_full_render: Instant,
    /// Window viewport size at the last full render. A change means the
    /// element's bounds are likely stale, so the next pump tick promotes a
    /// real notify instead of waiting out the frame interval.
    viewport_size_at_last_full_render: Option<Size<Pixels>>,
    /// Whether the `on_next_frame` pump has been started (once per view).
    pump_started: bool,
    active_placement: Option<(String, crate::scene_edit::SceneHistorySnapshot, (f32, f32))>,
    placement_monitor_running: bool,
    placement_bounds: Option<Bounds<Pixels>>,
    pending_placement: Option<(AssetPayload, (f32, f32))>,
    placement_retry_needed: bool,
}

mod assets;
mod frame_pump;
mod thumbnail;
use thumbnail::capture_viewport_png_tiled;
use thumbnail::capture_viewport_thumbnail;
impl HelioViewport {
    /// A view of `gpu_engine`, drawn by an engine runtime labelled `label`
    /// ("Level viewport", "Split viewport"). Higher `priority` draws first
    /// when several are due.
    pub fn new<V: 'static>(
        gpu_engine: Arc<Mutex<GpuRenderer>>,
        shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
        debug_replace_with_yellow: bool,
        label: &str,
        priority: i32,
        cx: &mut Context<V>,
    ) -> Self {
        use engine_backend::services::render_runtimes::{self, RuntimeDescription, UpdateMode};
        let scene_label = shared_state
            .read()
            .scene
            .current_scene
            .as_ref()
            .and_then(|path| path.file_stem())
            .map_or_else(
                || "level".to_string(),
                |stem| stem.to_string_lossy().into_owned(),
            );
        let runtime = render_runtimes::global().create(
            RuntimeDescription {
                label: label.to_string(),
                origin: pulsar_plugins::engine_origin!(),
                scene_label,
                priority,
                mode: UpdateMode::WhenVisible,
            },
            gpu_engine.clone(),
        );
        // The camera input is sampled as late as possible: right before the
        // frame reads it.
        runtime.set_before_frame(Box::new(|| crate::ui::viewport::input_latch::latch_now()));
        Self {
            gpu_engine,
            shared_state,
            surface: None,
            focus_handle: cx.focus_handle(),
            debug_replace_with_yellow,
            runtime,
            initial_load: None,
            editor_tab_active: true,
            dock_tab_active: true,
            frames_published: Arc::new(AtomicU64::new(0)),
            display_refresh_millihertz: Arc::new(AtomicU32::new(0)),
            last_display_refresh_sample: Instant::now(),
            last_published_frame: 0,
            last_loading_snapshot: None,
            last_screenshot_processing: false,
            awaiting_render: false,
            last_full_render: Instant::now(),
            viewport_size_at_last_full_render: None,
            pump_started: false,
            active_placement: None,
            placement_monitor_running: false,
            placement_bounds: None,
            pending_placement: None,
            placement_retry_needed: false,
        }
    }

    /// The level editor's tab (`editor`) or the view's own dock tab was shown
    /// or hidden. The view is shown when both are. Shown again, the next frame
    /// resets the temporal history (it would ghost from where the camera
    /// was); hidden, the runtime draws nothing.
    pub fn set_tab_active(&mut self, editor: bool, active: bool) {
        if editor {
            self.editor_tab_active = active;
        } else {
            self.dock_tab_active = active;
        }
        let visible = self.editor_tab_active && self.dock_tab_active;
        if visible {
            self.runtime.reset_history();
        }
        self.runtime.set_visible(visible);
    }

    /// Refresh rate of the display this window is on, falling back to the
    /// fastest connected display. Wayland has no primary output (winit's
    /// `primary_monitor()` is always `None` there) and a fresh window has no
    /// current output until the compositor sends `wl_surface.enter`, so both
    /// earlier fallbacks come up empty and would otherwise leave us at 60 Hz.
    fn sample_display_refresh_millihertz(window: &Window, cx: &App) -> Option<u32> {
        window
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
    }

    /// Re-sample the display refresh rate once per
    /// `DISPLAY_REFRESH_POLL_INTERVAL` until one is found, then stop.
    fn poll_display_refresh(&mut self, window: &Window, cx: &App) {
        if self.display_refresh_millihertz.load(Ordering::Relaxed) != 0
            || self.last_display_refresh_sample.elapsed() < DISPLAY_REFRESH_POLL_INTERVAL
        {
            return;
        }
        self.last_display_refresh_sample = Instant::now();
        if let Some(millihertz) = Self::sample_display_refresh_millihertz(window, cx) {
            self.display_refresh_millihertz
                .store(millihertz, Ordering::Relaxed);
        }
    }
}

const DISPLAY_REFRESH_POLL_INTERVAL: Duration = Duration::from_secs(1);

impl Focusable for HelioViewport {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for HelioViewport {}

impl Render for HelioViewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        profiling::profile_scope!("helio_viewport_frame");
        let (screenshot_processing, screenshot_progress) = {
            let build = &self.shared_state.read().build;
            (build.screenshot_processing, build.screenshot_progress)
        };
        if self.debug_replace_with_yellow {
            return div()
                .relative()
                .size_full()
                .track_focus(&self.focus_handle)
                .id("helio_viewport_debug_yellow")
                .bg(rgb(0xffff00))
                .into_any_element();
        }

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // The GPU device was lost (a driver reset, a removed GPU): stop
        // drawing on it. The window rebuilds itself on a new device and
        // renders every view again, and the surface made below is on that one;
        // the renderer follows the new device (`HelioRenderer` rebuilds on it).
        if self
            .surface
            .as_ref()
            .is_some_and(WgpuSurfaceHandle::is_device_lost)
        {
            tracing::warn!("[HELIO-VIEWPORT] GPU device lost; waiting for the window's new device");
            // The runtime marks itself lost from its output and builds again
            // on the next one.
            self.surface = None;
        }

        // Lazy surface creation; the runtime draws into it.
        if self.surface.is_none() {
            // If this comes up empty (Wayland before `wl_surface.enter`), the
            // frame pump keeps polling via `poll_display_refresh`.
            self.last_display_refresh_sample = Instant::now();
            let refresh_millihertz = Self::sample_display_refresh_millihertz(window, cx);
            if let Some(millihertz) = refresh_millihertz {
                self.display_refresh_millihertz
                    .store(millihertz, Ordering::Relaxed);
            }
            let refresh_hz = refresh_millihertz.map(|millihertz| millihertz as f64 / 1000.0);

            match window.create_wgpu_surface_with_color_conversion(
                1600,
                900,
                format,
                gpui::SurfaceColorConversion::LinearToSrgb,
            ) {
                // Until the window has rebuilt on a new device, a new surface
                // is on the lost one too; the rebuild renders this view again.
                Some(s) if s.is_device_lost() => {}
                Some(s) => {
                    tracing::info!("[HELIO-VIEWPORT] WgpuSurface created (format={:?})", format);
                    let _ = refresh_hz;
                    if self.initial_load.is_none() && self.runtime.frames() == 0 {
                        let loading = self
                            .shared_state
                            .read()
                            .editor_loading
                            .begin("Initializing Helio renderer…");
                        loading.set_progress(0.05);
                        self.initial_load = Some(loading);
                    }
                    // Let WGPUI present this frame before the runtime builds
                    // its cold pipeline graph on the shared device: the output
                    // is handed over at the start of the next platform frame.
                    let runtime = self.runtime.clone();
                    let target = Arc::new(ui_helio_viewport::SurfaceTarget::new(
                        s.clone(),
                        self.frames_published.clone(),
                        self.display_refresh_millihertz.clone(),
                    ));
                    window.on_next_frame(move |_, _| runtime.set_output(target));
                    self.surface = Some(s);
                }
                None => {
                    tracing::warn!("[HELIO-VIEWPORT] create_wgpu_surface returned None");
                }
            }
        }

        // Drain pending mesh-load errors (non-blocking).
        if let Ok(engine) = self.gpu_engine.try_lock() {
            for err in engine.drain_pending_errors() {
                window.push_notification(
                    Notification::error(t!("Notification.Title.MeshLoadFailed").to_string())
                        .message(err),
                    cx,
                );
            }
        }

        // Capture a project thumbnail if a save just requested one.
        // This must happen synchronously since it reads back GPU data.
        let capture_path = self
            .shared_state
            .write()
            .build
            .pending_thumbnail_capture
            .take();
        if let Some(path) = capture_path {
            if let Some(ref surface) = self.surface {
                if let Some((view, (w, h))) = surface.back_view_with_size() {
                    if let Ok(mut engine) = self.gpu_engine.try_lock() {
                        capture_viewport_thumbnail(&mut engine, surface, w, h, format, &path);
                    }
                }
            }
        }

        // Keep exclusive access to Helio across every tile so the projection,
        // scene state and frozen scene clock stay consistent until assembly ends.
        let screenshot_size = self
            .shared_state
            .write()
            .build
            .pending_screenshot_size
            .take();
        if let Some((width, height)) = screenshot_size {
            if let Some(surface) = self.surface.clone() {
                let engine = self.gpu_engine.clone();
                let state = self.shared_state.clone();
                let progress_state = state.clone();
                let temp_path = std::env::temp_dir().join(format!(
                    "pulsar-screenshot-{}-{}.png",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                let output_path = temp_path.clone();
                let capture = cx.background_executor().spawn(async move {
                    let mut engine = engine
                        .lock()
                        .map_err(|_| "Helio renderer lock was poisoned".to_string())?;
                    // Keep renderer panics inside this lock guard so they are
                    // reported as capture failures instead of poisoning the
                    // viewport renderer mutex or unwinding through the task.
                    catch_unwind(AssertUnwindSafe(|| {
                        capture_viewport_png_tiled(
                            &mut engine,
                            &surface,
                            width,
                            height,
                            format,
                            &output_path,
                            move |completed, total| {
                                progress_state.write().build.screenshot_progress =
                                    Some((completed, total));
                            },
                        )
                    }))
                    .map_err(|payload| {
                        let message = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&'static str>().copied())
                            .unwrap_or("unknown panic");
                        format!("Helio panicked while rendering a screenshot tile: {message}")
                    })??;
                    Ok::<_, String>(output_path)
                });
                cx.spawn_in(window, async move |this, cx| {
                    let result = capture.await;
                    {
                        let mut state = state.write();
                        state.build.screenshot_processing = false;
                        state.build.screenshot_progress = None;
                    }
                    let temp_path = match result {
                        Ok(path) => path,
                        Err(error) => {
                            tracing::warn!("High-resolution screenshot capture failed: {error}");
                            remove_temp_screenshot(&temp_path);
                            this.update_in(cx, |_, window, cx| {
                                window.push_notification(
                                    Notification::error("Screenshot capture failed").message(error),
                                    cx,
                                );
                            })
                            .ok();
                            return;
                        }
                    };
                    let dialog = rfd::AsyncFileDialog::new()
                        .set_title("Save High Res Screenshot")
                        .add_filter("PNG image", &["png"])
                        .set_file_name("screenshot.png");
                    if let Some(file) = dialog.save_file().await {
                        let path = file.path().to_path_buf();
                        if let Err(error) = copy_screenshot(&temp_path, &path) {
                            tracing::warn!("Failed to save high-resolution screenshot: {error}");
                            this.update_in(cx, |_, window, cx| {
                                window.push_notification(
                                    Notification::error("Could not save screenshot")
                                        .message(error.to_string()),
                                    cx,
                                );
                            })
                            .ok();
                        }
                    }
                    remove_temp_screenshot(&temp_path);
                })
                .detach();
            } else {
                let mut state = self.shared_state.write();
                state.build.screenshot_processing = false;
                state.build.screenshot_progress = None;
            }
        }

        // Build the viewport element
        let viewport_element = if let Some(surface) = self.surface.clone() {
            let (width, height) = surface.size();
            {
                let mut state = self.shared_state.write();
                state.build.viewport_size = (width, height);
                state.build.max_texture_dimension_2d =
                    surface.device().limits().max_texture_dimension_2d;
            }
            // Repaint this view whenever the render thread publishes a new frame,
            // so the compositor promotes `ready` -> `display` and
            // `WgpuSurface::prepaint` keeps observing the element's bounds (which
            // is what resizes the surface when the viewport panel changes size).
            //
            // The pump runs on `Window::on_next_frame`, which fires every platform
            // frame *without* marking anything dirty, so idle frames now cost
            // nothing instead of rebuilding the whole editor chrome.
            self.start_frame_pump(window, cx);
            // This render paints whatever the compositor promotes, so the pump's
            // outstanding request is satisfied and it may fire again. Prepaint of
            // the surface element below is also what re-observes bounds, so the
            // periodic-full-render counter restarts here.
            self.awaiting_render = false;
            self.last_full_render = Instant::now();
            self.viewport_size_at_last_full_render = Some(window.viewport_size());
            self.last_published_frame = self.frames_published.load(Ordering::Acquire);

            wgpu_surface(surface)
                .defer_resize_until_mouse_up(true)
                .absolute()
                .inset_0()
                .into_any_element()
        } else {
            // Surface not created yet: render nothing rather than a placeholder.
            // This lasts a single frame, so any overlay just reads as a flash.
            div()
                .relative()
                .track_focus(&self.focus_handle)
                .id("helio_viewport")
                .size_full()
                .into_any_element()
        };

        // Accept mesh/scene/blueprint payload drags and forward successful drops to the viewport entity.
        let viewport = cx.entity().clone();
        div()
            .id("helio-viewport-drop")
            .size_full()
            .on_drag_move::<AssetPayload>({
                let viewport = cx.entity().clone();
                move |event, window, cx| {
                    let payload = event.drag(cx).clone();
                    let local_x: f32 = (event.event.position.x - event.bounds.origin.x).into();
                    let local_y: f32 = (event.event.position.y - event.bounds.origin.y).into();
                    let width: f32 = event.bounds.size.width.into();
                    let height: f32 = event.bounds.size.height.into();
                    if width <= 0.0 || height <= 0.0 {
                        return;
                    }
                    let norm_x = local_x / width;
                    let norm_y = local_y / height;
                    if !(0.0..=1.0).contains(&norm_x) || !(0.0..=1.0).contains(&norm_y) {
                        return;
                    }
                    viewport.update(cx, |this, cx| {
                        this.placement_bounds = Some(event.bounds);
                        this.update_active_placement(&payload, norm_x, norm_y, window, cx)
                    });
                }
            })
            .drag_over::<AssetPayload>(|style, payload, _window, cx| {
                // Source models (FBX, …) are imported, not placed.
                if HelioViewport::supports_asset_placement(payload) {
                    style
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded(px(4.0))
                } else {
                    style.opacity(0.4)
                }
            })
            .on_drop::<AssetPayload>(move |payload, window, cx| {
                if payload.is_source_model() {
                    let payload = payload.clone();
                    viewport.update(cx, |this, cx| {
                        this.reject_source_model_drop(&payload, window, cx);
                    });
                } else if HelioViewport::supports_asset_placement(payload) {
                    let payload = payload.clone();
                    viewport.update(cx, |this, cx| {
                        this.handle_asset_drop(&payload, window, cx);
                    });
                }
            })
            .child(viewport_element)
            .when(screenshot_processing, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000099))
                        .text_color(rgb(0xffffff))
                        .child(match screenshot_progress {
                            Some((completed, total)) => format!(
                                "High resolution screenshot: {completed}/{total} tiles complete…"
                            ),
                            None => "High resolution screenshot is processing…".to_string(),
                        }),
                )
            })
            .into_any_element()
    }
}

// Allowed std::fs: the high-resolution screenshot is rendered to a temp file in
// the OS temp dir and copied to a path the user picked in a native save dialog
// on this machine; neither is a project file.
#[allow(clippy::disallowed_methods)]
fn copy_screenshot(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<u64> {
    std::fs::copy(from, to)
}

// Allowed std::fs: removes the screenshot's temp file from the OS temp dir.
#[allow(clippy::disallowed_methods)]
fn remove_temp_screenshot(path: &std::path::Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("Could not remove temporary screenshot: {error}");
        }
    }
}
