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
pub(crate) mod view_settings;
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
use ui::{v_flex, ActiveTheme};
use ui_common::ViewportControls;

use crate::state::LevelEditorState;
use crate::ui::viewport::components::camera_selector::CameraSpeedControl;
use components::gpu_pipeline_overlay::render_gpu_pipeline_overlay;
use components::nerds_overlay::{render_nerds_overlay, NerdsOverlay};
use components::performance_overlay::{render_performance_overlay, PerformanceOverlay};
use input_state::InputState;
use performance::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ViewportCursorCapture {
    #[default]
    Released,
    Rotate,
    Pan,
    PanRight,
}

impl ViewportCursorCapture {
    fn load(right: &AtomicBool, middle: &AtomicBool) -> Self {
        if right.load(Ordering::Acquire) {
            // Right-drag may pan when Shift is held. Preserve the initiating
            // button so Windows can stop capture if that button is released.
            if middle.load(Ordering::Acquire) {
                Self::PanRight
            } else {
                Self::Rotate
            }
        } else if middle.load(Ordering::Acquire) {
            Self::Pan
        } else {
            Self::Released
        }
    }

    fn store(self, right: &AtomicBool, middle: &AtomicBool) {
        right.store(
            matches!(self, Self::Rotate | Self::PanRight),
            Ordering::Release,
        );
        middle.store(
            matches!(self, Self::Pan | Self::PanRight),
            Ordering::Release,
        );
    }

    fn is_rotating(self) -> bool {
        self == Self::Rotate
    }

    fn is_panning(self) -> bool {
        matches!(self, Self::Pan | Self::PanRight)
    }

    #[cfg(target_os = "windows")]
    fn initiating_button_vk(self) -> i32 {
        if self == Self::Pan { 0x04 } else { 0x02 }
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
    toolbar: Entity<crate::ui::toolbar::ToolbarView>,

    /// The renderer this viewport draws with. Each viewport has its own
    /// (camera, gizmo and input state are per renderer); they all read one
    /// scene.
    gpu_engine: Arc<Mutex<GpuRenderer>>,

    /// What this viewport's own controls read and write: its overlays,
    /// camera mode and debug view. The editor's shared state for the primary
    /// viewport, a state of its own for a split-off one (see
    /// [`view_settings`]).
    view_state: Arc<parking_lot::RwLock<LevelEditorState>>,

    /// Whether `view_state` is a state of its own rather than the shared one.
    /// The shared state is already read-locked by the caller when the panel
    /// renders, so the panel must not lock it a second time.
    own_view: bool,

    /// Mailbox into `gpu_engine`, fetched once the renderer exists.
    mailbox: Option<engine_backend::subsystems::render::HelioEditorMailbox>,

    /// View-mode picker of a split-off viewport.
    own_picker: Option<view_settings::OwnViewPicker>,

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

    /// The render pipeline overlay, alive only while it is shown.
    gpu_overlay: RefCell<Option<Entity<components::gpu_pipeline_overlay::GpuPipelineOverlay>>>,

    /// Editor diagnostics overlay, alive only while it is shown.
    nerds_overlay: RefCell<Option<Entity<NerdsOverlay>>>,

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
    pub(crate) fn is_camera_navigating(&self) -> bool {
        ViewportCursorCapture::load(&self.mouse_right_captured, &self.mouse_middle_captured)
            .is_active()
    }

    /// Create a new viewport panel.
    pub fn new<V>(
        viewport: Entity<HelioViewport>,
        render_enabled: Arc<AtomicBool>,
        input_state: Arc<InputState>,
        toolbar: Entity<crate::ui::toolbar::ToolbarView>,
        gpu_engine: Arc<Mutex<GpuRenderer>>,
        view_state: Arc<parking_lot::RwLock<LevelEditorState>>,
        own_view: bool,
        _window: &mut Window,
        cx: &mut Context<V>,
    ) -> Self
    where
        V: 'static,
    {
        let focus_handle = cx.focus_handle();

        Self {
            viewport,
            toolbar,
            gpu_engine,
            view_state,
            own_view,
            mailbox: None,
            own_picker: None,
            cached_frame_snapshot: EngineFrameSnapshot::default(),
            viewport_controls: ViewportControls::new(),
            render_enabled,
            element_bounds: Rc::new(RefCell::new(None)),
            stats: SharedStats::default(),
            perf_overlay: RefCell::new(None),
            gpu_overlay: RefCell::new(None),
            nerds_overlay: RefCell::new(None),
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
            // MIN marks that the platform has no global cursor coordinate
            // (Wayland); (0, 0) and negative multi-monitor coordinates are valid.
            locked_cursor_screen_x: Arc::new(AtomicI32::new(i32::MIN)),
            locked_cursor_screen_y: Arc::new(AtomicI32::new(i32::MIN)),
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

        // Build the viewport UI
        self.build_viewport_ui(state, state_arc, snapshot, gpu_engine, cx)
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
