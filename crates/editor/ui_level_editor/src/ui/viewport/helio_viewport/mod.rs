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
    /// Keeps the runtime told where this window stands (focus, minimize).
    window_presence: Option<gpui::Subscription>,
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
mod render_steps;
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
            window_presence: None,
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
        self.drop_lost_surface();
        self.ensure_surface(window, cx, format);
        self.drain_mesh_load_errors(window, cx);
        self.capture_pending_thumbnail(format);
        self.start_pending_screenshot(window, cx, format);
        let viewport_element = self.viewport_element(window, cx);
        Self::asset_drop_target(
            viewport_element,
            screenshot_processing,
            screenshot_progress,
            cx,
        )
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
