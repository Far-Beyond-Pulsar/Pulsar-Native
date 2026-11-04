//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::{prelude::FluentBuilder as _, *};
use plugin_editor_api::{AssetKind, AssetPayload};
use rust_i18n::t;
use ui::{notification::Notification, ActiveTheme as _, ContextModal};

use crate::commands::{execute_command, SceneCommand, TypedComponent};
use crate::scene_edit::{MeshType, ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;
use helio_component::asset_component::component_for_asset;
use pulsar_reflection::REGISTRY;

/// A GPUI component that drives the Helio renderer into a `WgpuSurfaceHandle`.
///
/// Rendering runs on a dedicated background thread so the UI thread is never
/// blocked by GPU work. The background thread owns a clone of the `GpuRenderer`
/// and the `WgpuSurfaceHandle`; it loops at ~60 FPS calling
/// `render_frame_to_surface()` + `present_synced_silent()`.
///
/// Presentation is deliberately split across the two threads:
/// - the render thread only publishes finished frames into the triple buffer
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
    tab_activated: Arc<AtomicBool>,
    render_thread_stop: Arc<AtomicBool>,
    render_thread_handle: Option<std::thread::JoinHandle<()>>,
    /// Incremented by the render thread each time it publishes a frame into the
    /// triple buffer. Read by the frame pump to decide whether a repaint is
    /// warranted.
    frames_published: Arc<AtomicU64>,
    /// Refresh rate of the display the window is on, in millihertz (0 = not
    /// known yet). Written on the UI thread and picked up by the render
    /// thread's pacer, since on Wayland it's often unknown when the thread starts.
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
    last_spike_report: Instant,
    slow_frames_since_report: u32,
    engine_lock_misses_since_report: u32,
    max_frame_ms_since_report: f64,
    active_placement: Option<(String, crate::scene_edit::SceneHistorySnapshot, (f32, f32))>,
    placement_monitor_running: bool,
    placement_bounds: Option<Bounds<Pixels>>,
    pending_placement: Option<(AssetPayload, (f32, f32))>,
    placement_retry_needed: bool,
}

mod assets;
mod frame_pacer;
mod init_queue;
mod render_thread;
mod thumbnail;
use thumbnail::capture_viewport_image;
use thumbnail::capture_viewport_thumbnail;
impl HelioViewport {
    pub fn new<V: 'static>(
        gpu_engine: Arc<Mutex<GpuRenderer>>,
        shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
        debug_replace_with_yellow: bool,
        cx: &mut Context<V>,
    ) -> Self {
        Self {
            gpu_engine,
            shared_state,
            surface: None,
            focus_handle: cx.focus_handle(),
            debug_replace_with_yellow,
            tab_activated: Arc::new(AtomicBool::new(true)),
            render_thread_stop: Arc::new(AtomicBool::new(false)),
            render_thread_handle: None,
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
            last_spike_report: Instant::now(),
            slow_frames_since_report: 0,
            engine_lock_misses_since_report: 0,
            max_frame_ms_since_report: 0.0,
            active_placement: None,
            placement_monitor_running: false,
            placement_bounds: None,
            pending_placement: None,
            placement_retry_needed: false,
        }
    }

    /// Mark the viewport as having been activated (e.g. tab switch).
    /// The next render will reset TAA history to prevent ghosting.
    pub fn mark_tab_activated(&mut self) {
        self.tab_activated.store(true, Ordering::Release);
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
impl Drop for HelioViewport {
    fn drop(&mut self) {
        self.render_thread_stop.store(true, Ordering::Release);
    }
}

impl HelioViewport {
    /// Stop the render thread and wait for it, so the next one starts with
    /// the renderer to itself: a frame from the old thread on the lost
    /// device would rebuild the renderer back onto it.
    fn stop_render_thread(&mut self) {
        self.render_thread_stop.store(true, Ordering::Release);
        if let Some(handle) = self.render_thread_handle.take() {
            let _ = handle.join();
        }
        self.render_thread_stop = Arc::new(AtomicBool::new(false));
    }
}

impl Focusable for HelioViewport {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for HelioViewport {}

impl Render for HelioViewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        profiling::profile_scope!("helio_viewport_frame");
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
            self.stop_render_thread();
            self.surface = None;
        }

        // Lazy surface creation + start the background render thread.
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
                    // Let WGPUI finish presenting this frame before Helio asks
                    // the shared device to build its cold pipeline graph. The
                    // callback runs at the start of the next platform frame.
                    let (first_frame_tx, first_frame_rx) = std::sync::mpsc::channel();
                    window.on_next_frame(move |_, _| {
                        let _ = first_frame_tx.send(());
                    });
                    let loading = self
                        .shared_state
                        .read()
                        .editor_loading
                        .begin("Initializing Helio renderer…");
                    self.start_render_thread(s.clone(), refresh_hz, first_frame_rx, loading);
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
                    capture_viewport_png_tiled(
                        &mut engine,
                        &surface,
                        width,
                        height,
                        format,
                        &output_path,
                    )?;
                    Ok::<_, String>(output_path)
                });
                cx.spawn_in(window, async move |this, cx| {
                    let result = capture.await;
                    state.write().build.screenshot_processing = false;
                    let temp_path = match result {
                        Ok(path) => path,
                        Err(error) => {
                            tracing::warn!("High-resolution screenshot capture failed: {error}");
                            let _ = std::fs::remove_file(&temp_path);
                            this.update_in(cx, |_, window, cx| {
                                window.push_notification(
                                    Notification::error("Screenshot capture failed")
                                        .message(error),
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
                        if let Err(error) = std::fs::copy(&temp_path, &path) {
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
                    if let Err(error) = std::fs::remove_file(&temp_path) {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            tracing::warn!("Could not remove temporary screenshot: {error}");
                        }
                    }
                })
                .detach();
            } else {
                self.shared_state.write().build.screenshot_processing = false;
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
            .when(self.shared_state.read().build.screenshot_processing, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000099))
                        .text_color(rgb(0xffffff))
                        .child("High resolution screenshot is processing…"),
                )
            })
            .into_any_element()
    }
}
