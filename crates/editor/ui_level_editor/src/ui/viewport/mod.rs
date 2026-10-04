//! Viewport panel with 3D rendering and camera controls.
//!
//! This module provides the main viewport panel for the level editor, featuring:
//! - Direct GPU rendering via Helio
//! - Professional camera controls (FPS, pan, orbit, zoom)
//! - Performance monitoring and overlays
//! - Lock-free input processing on dedicated thread
//!
//! The viewport has been refactored into focused, reusable components for maintainability.

mod build;
mod build_handlers;
pub mod components;
pub mod cursor;
pub mod game_viewport;
pub mod helio_viewport;
pub(crate) mod input_latch;
pub mod input_state;
mod input_thread;
mod overlays;
pub mod performance;

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use device_query::{DeviceQuery, DeviceState, Keycode};
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use helio_viewport::HelioViewport;
use ui::Sizable;
use ui::{ActiveTheme, v_flex};
use ui_common::ViewportControls;

use crate::state::LevelEditorState;
use crate::ui::viewport::components::camera_selector::CameraSpeedControl;
use components::camera_selector::render_camera_selector;
use components::gpu_pipeline_overlay::render_gpu_pipeline_overlay;
use components::performance_overlay::{PerformanceOverlay, render_performance_overlay};
use components::viewport_options::render_viewport_options;
use input_state::InputState;
use performance::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ViewportCursorCapture {
    #[default]
    Released,
    Rotate,
    Pan,
}

impl ViewportCursorCapture {
    fn load(right: &AtomicBool, middle: &AtomicBool) -> Self {
        if right.load(Ordering::Acquire) {
            Self::Rotate
        } else if middle.load(Ordering::Acquire) {
            Self::Pan
        } else {
            Self::Released
        }
    }

    fn store(self, right: &AtomicBool, middle: &AtomicBool) {
        right.store(matches!(self, Self::Rotate), Ordering::Release);
        middle.store(matches!(self, Self::Pan), Ordering::Release);
    }

    fn is_active(self) -> bool {
        self != Self::Released
    }
}

/// Route one viewport pointer event through the active tool mode (design doc
/// §4.5).
///
/// All three pointer handlers below funnel through here so the dispatch
/// contract stays in one place: `Consumed` means the mode fully handled the
/// event and the default pick/gizmo mailbox path must be skipped;
/// `PassThrough` means carry on exactly as before. `LevelEditMode` always
/// returns `PassThrough`, so the default mode's behavior is unchanged.
#[allow(clippy::too_many_arguments)]
fn dispatch_tool_pointer(
    state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: &Arc<Mutex<GpuRenderer>>,
    camera: crate::tool_modes::CameraFrame,
    viewport_size: (f32, f32),
    kind: crate::tool_modes::PointerKind,
    button: Option<gpui::MouseButton>,
    norm_x: f32,
    norm_y: f32,
    modifiers: gpui::Modifiers,
) -> crate::tool_modes::ToolPointerResult {
    let pointer_event = crate::tool_modes::ToolPointerEvent {
        kind,
        button,
        norm_x,
        norm_y,
        holding_mods: modifiers,
    };
    let viewport_frame = crate::tool_modes::ViewportFrame {
        width: viewport_size.0,
        height: viewport_size.1,
    };
    let mut state = state_arc.write();
    crate::tool_modes::ToolModeDispatcher::dispatch_pointer(
        &mut state,
        gpu_engine,
        &pointer_event,
        camera,
        viewport_frame,
    )
}

/// Build a [`CameraFrame`](crate::tool_modes::CameraFrame) from
/// the renderer's editor camera. Falls back to the default frame when the
/// state is unavailable, which makes any ray built from it miss -- the correct
/// failure mode, since a wrong camera would place the brush somewhere the user
/// did not click.
fn tool_camera_frame(
    state: Option<engine_backend::subsystems::render::EditorCameraState>,
) -> crate::tool_modes::CameraFrame {
    state
        .map(|c| crate::tool_modes::CameraFrame {
            position: c.position.map(|coordinate| coordinate as f32),
            yaw: c.yaw,
            pitch: c.pitch,
            fov: 60.0,
        })
        .unwrap_or_default()
}

/// Viewport panel with zero-copy GPU rendering and professional camera controls.
///
/// This panel manages:
/// - Direct GPU rendering through Helio (no CPU copies)
/// - Dedicated input thread for high-frequency polling
/// - Performance metrics tracking and visualization
/// - Camera mode selection and controls
/// - Visual option toggles (grid, wireframe, lighting)
pub struct ViewportPanel {
    cached_frame_snapshot: EngineFrameSnapshot,
    /// Helio viewport entity for GPU rendering
    viewport: Entity<HelioViewport>,

    /// Viewport controls state
    viewport_controls: ViewportControls,

    /// Render enable/disable flag
    render_enabled: Arc<AtomicBool>,

    /// Element bounds for coordinate conversion
    element_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,

    /// Per-frame performance samples, read by the overlay's sections at their
    /// own refresh rates.
    stats: SharedStats,

    /// The performance overlay, alive only while it is shown (so its refresh
    /// timers run only then).
    perf_overlay: RefCell<Option<Entity<PerformanceOverlay>>>,

    /// Lock-free input state
    input_state: Arc<InputState>,

    /// Input thread spawn tracking
    input_thread_spawned: Arc<AtomicBool>,

    /// Signals the input thread to exit
    input_thread_stop: Arc<AtomicBool>,

    /// Join handle for the input thread
    input_thread_handle: Option<std::thread::JoinHandle<()>>,

    /// Viewport hover state
    viewport_hovered: Arc<AtomicBool>,

    /// Mouse tracking (all atomic for lock-free access)
    last_mouse_x: Arc<AtomicI32>,
    last_mouse_y: Arc<AtomicI32>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,

    /// Locked cursor position for infinite mouse movement
    locked_cursor_x: Arc<AtomicI32>,
    locked_cursor_y: Arc<AtomicI32>,
    locked_cursor_screen_x: Arc<AtomicI32>,
    locked_cursor_screen_y: Arc<AtomicI32>,

    /// Keyboard state
    keys_pressed: Rc<RefCell<HashSet<String>>>,
    alt_pressed: Rc<RefCell<bool>>,

    /// Focus handle
    focus_handle: FocusHandle,
}

impl ViewportPanel {
    /// Create a new viewport panel.
    pub fn new<V>(
        viewport: Entity<HelioViewport>,
        render_enabled: Arc<AtomicBool>,
        _window: &mut Window,
        cx: &mut Context<V>,
    ) -> Self
    where
        V: 'static,
    {
        let input_state = Arc::new(InputState::new());
        let focus_handle = cx.focus_handle();

        Self {
            viewport,
            cached_frame_snapshot: EngineFrameSnapshot::default(),
            viewport_controls: ViewportControls::new(),
            render_enabled,
            element_bounds: Rc::new(RefCell::new(None)),
            stats: SharedStats::default(),
            perf_overlay: RefCell::new(None),
            input_state,
            input_thread_spawned: Arc::new(AtomicBool::new(false)),
            input_thread_stop: Arc::new(AtomicBool::new(false)),
            input_thread_handle: None,
            viewport_hovered: Arc::new(AtomicBool::new(false)),
            last_mouse_x: Arc::new(AtomicI32::new(0)),
            last_mouse_y: Arc::new(AtomicI32::new(0)),
            mouse_right_captured: Arc::new(AtomicBool::new(false)),
            mouse_middle_captured: Arc::new(AtomicBool::new(false)),
            locked_cursor_x: Arc::new(AtomicI32::new(0)),
            locked_cursor_y: Arc::new(AtomicI32::new(0)),
            locked_cursor_screen_x: Arc::new(AtomicI32::new(0)),
            locked_cursor_screen_y: Arc::new(AtomicI32::new(0)),
            keys_pressed: Rc::new(RefCell::new(HashSet::new())),
            alt_pressed: Rc::new(RefCell::new(false)),
            focus_handle,
        }
    }

    /// Render the viewport panel.
    pub fn render<V>(
        &mut self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        // Spawn dedicated input thread (once)
        self.spawn_input_thread_once(gpu_engine);

        let snapshot = EngineFrameSnapshot::gather(
            gpu_engine,
            self.input_state.get_move_speed(),
            self.input_state.take_zoom_delta(),
        );

        // Update performance metrics
        self.update_performance_metrics(
            snapshot.as_ref(),
            state.overlays.state.show_performance_overlay
                && !state.overlays.state.performance_overlay_collapsed,
        );

        // Build the viewport UI
        self.build_viewport_ui(state, state_arc, snapshot, gpu_engine, cx)
    }
}

impl ViewportPanel {
    /// Record this frame's samples for the performance overlay.
    ///
    /// Only while the overlay is open: with it closed nothing reads them. This
    /// is all the viewport does for the overlay each frame (a lock and a few
    /// additions); the sections pull what they show at their own rates.
    fn update_performance_metrics(
        &self,
        snapshot: Option<&EngineFrameSnapshot>,
        overlay_open: bool,
    ) {
        if !overlay_open {
            return;
        }
        let mut stats = self.stats.lock();

        if let Some(snapshot) = snapshot {
            // The renderer's metric stands in when the UI-side frame count is
            // not available yet.
            let ui_fps = if snapshot.ui_fps > 0.0 {
                snapshot.ui_fps
            } else {
                snapshot.helio_fps
            };
            stats.record(Metric::UiFps, ui_fps);
            stats.record(Metric::RenderFps, snapshot.render_fps);
            stats.record(Metric::FrameTimeMs, snapshot.frame_time_ms);
            stats.record(Metric::DrawCalls, snapshot.draw_calls);
            stats.record(Metric::Vertices, snapshot.vertices);
            stats.record(Metric::MemoryMb, snapshot.memory_mb);
        }

        let latency_us = self.input_state.get_input_latency_us();
        stats.record(Metric::InputLatencyMs, latency_us as f64 / 1000.0);
    }
}

impl Drop for ViewportPanel {
    fn drop(&mut self) {
        self.input_thread_stop.store(true, Ordering::Release);
        if let Some(handle) = self.input_thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Everything `render_viewport_options` reads out of [`LevelEditorState`].
///
/// This is a claim, and the failure mode of getting it wrong is a toolbar that
/// stops updating — so it is derived by reading that function and its three
/// helpers (`visual_toggles`, `gizmo_tool_buttons`, `overlay_toggles`) rather
/// than by guessing, and it must be revisited whenever one of them starts
/// reading something new. `WGPUI_LAYER_DEBUG=1` makes an omission visible: the
/// layer fails to flash when the thing it forgot changes.
///
/// The theme is deliberately absent. It is a global rather than an entity, so
/// nothing here could observe it — but changing it goes through
/// `Window::refresh`, which is window-scope invalidation on every axis, and no
/// layer composites through that.
fn viewport_options_key(state: &LevelEditorState) -> impl std::hash::Hash {
    (
        state.overlays.state.viewport_options_collapsed,
        state.overlays.state.show_performance_overlay,
        state.overlays.state.show_gpu_pipeline_overlay,
        state.editor.show_grid,
        state.editor.show_wireframe,
        state.editor.show_lighting,
        state.editor.current_tool as u8,
        state.overlays.positions.is_dragging_viewport,
    )
}

#[cfg(test)]
mod tests {
    use super::ViewportCursorCapture;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn cursor_capture_modes_store_exclusive_flags() {
        let right = AtomicBool::new(false);
        let middle = AtomicBool::new(false);

        for mode in [
            ViewportCursorCapture::Rotate,
            ViewportCursorCapture::Pan,
            ViewportCursorCapture::Released,
        ] {
            mode.store(&right, &middle);
            assert_eq!(ViewportCursorCapture::load(&right, &middle), mode);
            assert_eq!(mode.is_active(), mode != ViewportCursorCapture::Released);
            assert!(!(right.load(Ordering::Acquire) && middle.load(Ordering::Acquire)));
        }
    }

    #[test]
    fn cursor_capture_load_resolves_legacy_conflict_to_rotation() {
        let right = AtomicBool::new(true);
        let middle = AtomicBool::new(true);

        assert_eq!(
            ViewportCursorCapture::load(&right, &middle),
            ViewportCursorCapture::Rotate
        );
    }
}
