//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use plugin_editor_api::{AssetKind, AssetPayload};
use rust_i18n::t;
use ui::{notification::Notification, ActiveTheme as _, ContextModal};

use crate::level_editor::commands::{execute_command, SceneCommand};
use crate::level_editor::scene_edit::{MeshType, ObjectType, SceneObjectData, Transform};
use crate::level_editor::state::LevelEditorState;
use helio_component::asset_component::component_class_for_asset;
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
    /// Value of `frames_published` at the last repaint this view requested.
    last_published_frame: u64,
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
}

mod assets;
mod frame_pacer;
mod render_thread;
mod thumbnail;
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
            last_published_frame: 0,
            awaiting_render: false,
            last_full_render: Instant::now(),
            viewport_size_at_last_full_render: None,
            pump_started: false,
            last_spike_report: Instant::now(),
            slow_frames_since_report: 0,
            engine_lock_misses_since_report: 0,
            max_frame_ms_since_report: 0.0,
        }
    }

    /// Mark the viewport as having been activated (e.g. tab switch).
    /// The next render will reset TAA history to prevent ghosting.
    pub fn mark_tab_activated(&mut self) {
        self.tab_activated.store(true, Ordering::Release);
    }
}
impl Drop for HelioViewport {
    fn drop(&mut self) {
        self.render_thread_stop.store(true, Ordering::Release);
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

        // Lazy surface creation (once) + start the background render thread.
        if self.surface.is_none() {
            // Read the refresh rate here, on the UI thread, while we still have
            // a `Window`: it's what the render thread starts its pacing at.
            let refresh_hz = window
                .display(cx)
                .and_then(|display| display.refresh_rate_millihertz())
                .map(|mhz| mhz as f64 / 1000.0)
                .filter(|hz| *hz > 0.0);

            match window.create_wgpu_surface_with_color_conversion(
                1600,
                900,
                format,
                gpui::SurfaceColorConversion::LinearToSrgb,
            ) {
                Some(s) => {
                    tracing::info!("[HELIO-VIEWPORT] WgpuSurface created (format={:?})", format);
                    self.start_render_thread(s.clone(), refresh_hz);
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

        // Build the viewport element
        let viewport_element = if let Some(surface) = self.surface.clone() {
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
            .drag_over::<AssetPayload>(|style, payload, _window, cx| {
                if matches!(
                    payload.kind,
                    AssetKind::Mesh | AssetKind::Scene | AssetKind::Blueprint
                ) {
                    style
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded(px(4.0))
                } else {
                    style.opacity(0.4)
                }
            })
            .on_drop::<AssetPayload>(move |payload, window, cx| {
                if matches!(
                    payload.kind,
                    AssetKind::Mesh | AssetKind::Scene | AssetKind::Blueprint
                ) {
                    let payload = payload.clone();
                    viewport.update(cx, |this, cx| {
                        this.handle_asset_drop(&payload, window, cx);
                    });
                }
            })
            .child(viewport_element)
            .into_any_element()
    }
}
