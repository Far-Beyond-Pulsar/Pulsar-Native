use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::{Button, ButtonVariants as _}, h_flex, v_flex, ActiveTheme, IconName, Selectable, Sizable, StyledExt,
    chart::{BarChart, AreaChart},
    PixelsExt,
};
// Zero-copy Bevy viewport for 3D rendering
use ui::bevy_viewport::BevyViewport;

use super::state::{CameraMode, LevelEditorState};
use super::actions::*;
use ui_common::ViewportControls;
use engine_backend::GameThread;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::collections::VecDeque;
use std::cell::RefCell;
use std::rc::Rc;

// Raw input polling for viewport controls (cross-platform)
use device_query::{DeviceQuery, DeviceState, Keycode};

// Windows API for cursor locking and raw input
#[cfg(target_os = "windows")]
use winapi::um::winuser::{ClipCursor, GetClientRect, ClientToScreen, GetCursorPos, SetCursorPos, ShowCursor};
#[cfg(target_os = "windows")]
use winapi::shared::windef::{RECT, POINT};
#[cfg(target_os = "windows")]
use winapi::shared::minwindef::FALSE;

// Windows-specific cursor locking and repositioning functions
#[cfg(target_os = "windows")]
fn lock_cursor_to_window(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // Get the window handle from GPUI Window directly
    match HasWindowHandle::window_handle(window) {
        Ok(handle) => {
            match handle.as_raw() {
                RawWindowHandle::Win32(win32_handle) => {
                    unsafe {
                        let hwnd = win32_handle.hwnd.get() as *mut winapi::shared::windef::HWND__;

                        // Get client rect
                        let mut client_rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                        if GetClientRect(hwnd, &mut client_rect) != 0 {
                            // Convert client coordinates to screen coordinates
                            let mut top_left = POINT { x: client_rect.left, y: client_rect.top };
                            let mut bottom_right = POINT { x: client_rect.right, y: client_rect.bottom };

                            ClientToScreen(hwnd, &mut top_left);
                            ClientToScreen(hwnd, &mut bottom_right);

                            // Create screen rect
                            let screen_rect = RECT {
                                left: top_left.x,
                                top: top_left.y,
                                right: bottom_right.x,
                                bottom: bottom_right.y,
                            };

                            // Confine cursor to window bounds
                            ClipCursor(&screen_rect);
                            tracing::info!("[VIEWPORT] 🔒 Cursor locked to window bounds");
                        }
                    }
                }
                _ => {
                    tracing::warn!("[VIEWPORT] Not a Win32 window handle");
                }
            }
        }
        Err(e) => {
            tracing::error!("[VIEWPORT] Failed to get window handle: {:?}", e);
        }
    }
}

/// Lock cursor to a small area around a specific point (Windows)
/// This prevents the cursor from escaping during fast movements
#[cfg(target_os = "windows")]
fn lock_cursor_to_point(screen_x: i32, screen_y: i32, radius: i32) {
    unsafe {
        let screen_rect = RECT {
            left: screen_x - radius,
            top: screen_y - radius,
            right: screen_x + radius,
            bottom: screen_y + radius,
        };
        ClipCursor(&screen_rect);
        tracing::info!("[VIEWPORT] 🔒 Cursor confined to {}px radius around ({}, {})", radius, screen_x, screen_y);
    }
}

#[cfg(not(target_os = "windows"))]
fn lock_cursor_to_point(_screen_x: i32, _screen_y: i32, _radius: i32) {
    // No-op on non-Windows (we rely on cursor repositioning)
}

#[cfg(target_os = "windows")]
fn unlock_cursor() {
    unsafe {
        // Pass NULL to release cursor confinement
        ClipCursor(std::ptr::null());
        tracing::info!("[VIEWPORT] 🔓 Cursor unlocked");
    }
}

/// Hide cursor (Windows)
#[cfg(target_os = "windows")]
fn hide_cursor() {
    unsafe {
        // ShowCursor is counter-based - call until cursor is hidden
        // The cursor is hidden when the counter < 0
        while ShowCursor(FALSE) >= 0 {}
        tracing::info!("[VIEWPORT] 👻 Cursor hidden (Win32 ShowCursor)");
    }
}

/// Show cursor (Windows)
#[cfg(target_os = "windows")]
fn show_cursor() {
    unsafe {
        // ShowCursor is counter-based - call until cursor is shown
        // The cursor is shown when the counter >= 0
        while ShowCursor(1) < 0 {}
        tracing::info!("[VIEWPORT] 👁️ Cursor shown (Win32 ShowCursor)");
    }
}

/// Set cursor to absolute screen position (Windows)
#[cfg(target_os = "windows")]
fn set_cursor_position(screen_x: i32, screen_y: i32) {
    unsafe {
        SetCursorPos(screen_x, screen_y);
    }
}

/// Convert window-relative position to screen position (Windows)
#[cfg(target_os = "windows")]
fn window_to_screen_position(window: &Window, window_x: f32, window_y: f32) -> Option<(i32, i32)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => {
            match handle.as_raw() {
                RawWindowHandle::Win32(win32_handle) => {
                    unsafe {
                        let hwnd = win32_handle.hwnd.get() as *mut winapi::shared::windef::HWND__;
                        let mut point = POINT {
                            x: window_x as i32,
                            y: window_y as i32
                        };
                        ClientToScreen(hwnd, &mut point);
                        Some((point.x, point.y))
                    }
                }
                _ => None,
            }
        }
        Err(_) => None,
    }
}

/// Set cursor to absolute screen position (macOS)
#[cfg(target_os = "macos")]
fn set_cursor_position(screen_x: i32, screen_y: i32) {
    use core_graphics::display::CGDisplay;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    unsafe {
        CGDisplay::warp_mouse_cursor_position(core_graphics::geometry::CGPoint {
            x: screen_x as f64,
            y: screen_y as f64,
        }).ok();

        // Disassociate mouse and cursor position momentarily to prevent the cursor
        // from jumping back
        CGEventSource::set_local_events_suppression_interval(0.0);
    }
}

/// Convert window-relative position to screen position (macOS)
#[cfg(target_os = "macos")]
fn window_to_screen_position(window: &Window, window_x: f32, window_y: f32) -> Option<(i32, i32)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::NSPoint;

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => {
            match handle.as_raw() {
                RawWindowHandle::AppKit(appkit_handle) => {
                    unsafe {
                        let ns_window = appkit_handle.ns_window.as_ptr() as *mut AnyObject;
                        let point = NSPoint {
                            x: window_x as f64,
                            y: window_y as f64,
                        };
                        let screen_point: NSPoint = msg_send![ns_window, convertPointToScreen: point];
                        Some((screen_point.x as i32, screen_point.y as i32))
                    }
                }
                _ => None,
            }
        }
        Err(_) => None,
    }
}

/// Set cursor to absolute screen position (Linux)
#[cfg(target_os = "linux")]
fn set_cursor_position(screen_x: i32, screen_y: i32) {
    // TODO: Implement for X11/Wayland
    tracing::warn!("[VIEWPORT] Cursor repositioning not yet implemented for Linux");
}

/// Convert window-relative position to screen position (Linux)
#[cfg(target_os = "linux")]
fn window_to_screen_position(_window: &Window, _window_x: f32, _window_y: f32) -> Option<(i32, i32)> {
    // TODO: Implement for X11/Wayland
    None
}

#[cfg(not(target_os = "windows"))]
fn hide_cursor() {
    // Will use GPUI's cursor style on other platforms
}

#[cfg(not(target_os = "windows"))]
fn show_cursor() {
    // Will use GPUI's cursor style on other platforms
}

#[cfg(not(target_os = "windows"))]
fn lock_cursor_to_window(_window: &Window) {
    // macOS and Linux don't need window confinement for our use case
    // They use cursor repositioning instead
    tracing::info!("[VIEWPORT] Cursor locking via repositioning");
}

#[cfg(not(target_os = "windows"))]
fn unlock_cursor() {
    // No-op on non-Windows platforms (we use cursor repositioning instead)
}

/// Lock-free input state using atomics - no mutex contention!
#[derive(Clone)]
struct InputState {
    // Keyboard movement (atomic for lock-free access)
    forward: Arc<AtomicI32>,  // -1, 0, 1
    right: Arc<AtomicI32>,    // -1, 0, 1
    up: Arc<AtomicI32>,       // -1, 0, 1
    boost: Arc<AtomicBool>,

    // Mouse position (stored as i32 * 1000 for fractional precision)
    mouse_x: Arc<AtomicI32>,
    mouse_y: Arc<AtomicI32>,

    // Mouse deltas (stored as i32 * 1000 for fractional precision)
    mouse_delta_x: Arc<AtomicI32>,
    mouse_delta_y: Arc<AtomicI32>,
    pan_delta_x: Arc<AtomicI32>,
    pan_delta_y: Arc<AtomicI32>,
    zoom_delta: Arc<AtomicI32>,

    // Move speed adjustment
    move_speed: Arc<AtomicI32>, // * 100 for precision

    // Input latency tracking (measured on input thread)
    // Stores microseconds since last input was received, as i64
    input_latency_us: Arc<std::sync::atomic::AtomicU64>,
}

impl InputState {
    fn new() -> Self {
        Self {
            forward: Arc::new(AtomicI32::new(0)),
            right: Arc::new(AtomicI32::new(0)),
            up: Arc::new(AtomicI32::new(0)),
            boost: Arc::new(AtomicBool::new(false)),
            mouse_x: Arc::new(AtomicI32::new(0)),
            mouse_y: Arc::new(AtomicI32::new(0)),
            mouse_delta_x: Arc::new(AtomicI32::new(0)),
            mouse_delta_y: Arc::new(AtomicI32::new(0)),
            pan_delta_x: Arc::new(AtomicI32::new(0)),
            pan_delta_y: Arc::new(AtomicI32::new(0)),
            zoom_delta: Arc::new(AtomicI32::new(0)),
            move_speed: Arc::new(AtomicI32::new(1000)), // 10.0 * 100
            input_latency_us: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
    
    // Set mouse delta (converts f32 to i32 * 1000)
    fn set_mouse_delta(&self, x: f32, y: f32) {
        self.mouse_delta_x.store((x * 1000.0) as i32, Ordering::Relaxed);
        self.mouse_delta_y.store((y * 1000.0) as i32, Ordering::Relaxed);
    }
    
    fn set_pan_delta(&self, x: f32, y: f32) {
        self.pan_delta_x.store((x * 1000.0) as i32, Ordering::Relaxed);
        self.pan_delta_y.store((y * 1000.0) as i32, Ordering::Relaxed);
    }
    
    fn set_zoom_delta(&self, z: f32) {
        self.zoom_delta.store((z * 1000.0) as i32, Ordering::Relaxed);
    }
    
    fn adjust_move_speed(&self, delta: f32) {
        let current = self.move_speed.load(Ordering::Relaxed) as f32 / 100.0;
        let new_speed = (current + delta).clamp(0.5, 100.0);
        self.move_speed.store((new_speed * 100.0) as i32, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct FpsDataPoint {
    index: usize,
    fps: f64,
}

#[derive(Clone)]
struct TpsDataPoint {
    index: usize,
    tps: f64,
}

#[derive(Clone)]
struct FrameTimeDataPoint {
    index: usize,
    frame_time_ms: f64,
}

#[derive(Clone)]
struct MemoryDataPoint {
    index: usize,
    memory_mb: f64,
}

#[derive(Clone)]
struct DrawCallsDataPoint {
    index: usize,
    draw_calls: f64,
}

#[derive(Clone)]
struct VerticesDataPoint {
    index: usize,
    vertices: f64,
}

#[derive(Clone)]
struct InputLatencyDataPoint {
    index: usize,
    latency_ms: f64,
}

#[derive(Clone)]
struct UiConsistencyDataPoint {
    index: usize,
    consistency_score: f64, // Lower is better - represents stddev of FPS
}

#[derive(Clone, Copy, PartialEq)]
enum GraphType {
    Line,
    Bar,
}

/// Viewport Panel - TRUE ZERO-COPY GPU 3D rendering viewport with PRO camera controls
/// Studio-quality navigation: FPS mode, Pan, Orbit, Zoom
/// Direct GPU rendering - NO CPU COPIES!
/// Input is processed on a dedicated input thread with latency tracking
pub struct ViewportPanel {
    viewport: Entity<BevyViewport>,
    viewport_controls: ViewportControls,
    render_enabled: Arc<std::sync::atomic::AtomicBool>,
    // Element bounds for accurate coordinate conversion (window → element)
    element_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    // FPS tracking for rolling graph - using RefCell for interior mutability
    fps_history: RefCell<VecDeque<FpsDataPoint>>,
    fps_sample_counter: RefCell<usize>,
    // TPS tracking for rolling graph
    tps_history: RefCell<VecDeque<TpsDataPoint>>,
    tps_sample_counter: RefCell<usize>,
    // Frame time tracking for jitter analysis
    frame_time_history: RefCell<VecDeque<FrameTimeDataPoint>>,
    frame_time_counter: RefCell<usize>,
    // Memory usage tracking
    memory_history: RefCell<VecDeque<MemoryDataPoint>>,
    memory_counter: RefCell<usize>,
    // Draw calls tracking
    draw_calls_history: RefCell<VecDeque<DrawCallsDataPoint>>,
    draw_calls_counter: RefCell<usize>,
    // Vertices tracking
    vertices_history: RefCell<VecDeque<VerticesDataPoint>>,
    vertices_counter: RefCell<usize>,
    // Input latency tracking (measured on input thread)
    input_latency_history: RefCell<VecDeque<InputLatencyDataPoint>>,
    input_latency_counter: RefCell<usize>,
    // UI refresh consistency tracking (tracks FPS variance over time)
    ui_consistency_history: RefCell<VecDeque<UiConsistencyDataPoint>>,
    ui_consistency_counter: RefCell<usize>,
    // Lock-free input state - no mutex contention on UI thread!
    input_state: InputState,
    // Track if input thread has been spawned
    input_thread_spawned: Arc<AtomicBool>,
    // Track if viewport is hovered/focused - only process input when true
    viewport_hovered: Arc<AtomicBool>,
    // Mouse tracking - ALL ATOMIC! No RefCell!
    last_mouse_x: Arc<AtomicI32>,
    last_mouse_y: Arc<AtomicI32>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    // Locked cursor position for infinite mouse movement during drag (window-relative, * 1000)
    locked_cursor_x: Arc<AtomicI32>,
    locked_cursor_y: Arc<AtomicI32>,
    // Locked cursor position in screen coordinates for repositioning
    locked_cursor_screen_x: Arc<AtomicI32>,
    locked_cursor_screen_y: Arc<AtomicI32>,
    // Keyboard state for WASD + modifiers - NOT NEEDED ANYMORE, using atomics directly!
    keys_pressed: Rc<RefCell<std::collections::HashSet<String>>>,
    alt_pressed: Rc<RefCell<bool>>,
    // Focus handle for input
    focus_handle: FocusHandle,
}

impl ViewportPanel {
    pub fn new<V>(viewport: Entity<BevyViewport>, render_enabled: Arc<std::sync::atomic::AtomicBool>, window: &mut Window, cx: &mut Context<V>) -> Self
    where
        V: 'static,
    {
        let input_state = InputState::new();
        let focus_handle = cx.focus_handle();
        
        // No focus subscriptions needed - we track button state directly in input thread
        
        Self {
            viewport,
            viewport_controls: ViewportControls::new(),
            render_enabled,
            element_bounds: Rc::new(RefCell::new(None)),
            fps_history: RefCell::new(VecDeque::with_capacity(120)),
            fps_sample_counter: RefCell::new(0),
            tps_history: RefCell::new(VecDeque::with_capacity(120)),
            tps_sample_counter: RefCell::new(0),
            frame_time_history: RefCell::new(VecDeque::with_capacity(120)),
            frame_time_counter: RefCell::new(0),
            memory_history: RefCell::new(VecDeque::with_capacity(120)),
            memory_counter: RefCell::new(0),
            draw_calls_history: RefCell::new(VecDeque::with_capacity(120)),
            draw_calls_counter: RefCell::new(0),
            vertices_history: RefCell::new(VecDeque::with_capacity(120)),
            vertices_counter: RefCell::new(0),
            input_latency_history: RefCell::new(VecDeque::with_capacity(120)),
            input_latency_counter: RefCell::new(0),
            ui_consistency_history: RefCell::new(VecDeque::with_capacity(120)),
            ui_consistency_counter: RefCell::new(0),
            input_state,
            input_thread_spawned: Arc::new(AtomicBool::new(false)),
            viewport_hovered: Arc::new(AtomicBool::new(false)),
            last_mouse_x: Arc::new(AtomicI32::new(0)),
            last_mouse_y: Arc::new(AtomicI32::new(0)),
            mouse_right_captured: Arc::new(AtomicBool::new(false)),
            mouse_middle_captured: Arc::new(AtomicBool::new(false)),
            locked_cursor_x: Arc::new(AtomicI32::new(0)),
            locked_cursor_y: Arc::new(AtomicI32::new(0)),
            locked_cursor_screen_x: Arc::new(AtomicI32::new(0)),
            locked_cursor_screen_y: Arc::new(AtomicI32::new(0)),
            keys_pressed: Rc::new(RefCell::new(std::collections::HashSet::new())),
            alt_pressed: Rc::new(RefCell::new(false)),
            focus_handle,
        }
    }

    pub fn render<V: 'static>(
        &mut self,
        state: &mut LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        fps_graph_state: Rc<RefCell<bool>>,  // Shared state for the Switch
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        game_thread: &Arc<GameThread>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        // Note: We can't check focus here because we don't have Window reference in this context
        // Instead, the viewport div will set focus when clicked (via track_focus)
        // and the input thread will check if right button is pressed to determine activity
        
        // Spawn dedicated input processing thread ONLY ONCE (not every frame!)
        if !self.input_thread_spawned.load(Ordering::Relaxed) {
            self.input_thread_spawned.store(true, Ordering::Relaxed);
            
            let input_state_for_thread = self.input_state.clone();
            let gpu_engine_for_thread = gpu_engine.clone();
            let mouse_right_captured = self.mouse_right_captured.clone();
            let mouse_middle_captured = self.mouse_middle_captured.clone();
            let locked_cursor_x = self.locked_cursor_x.clone();
            let locked_cursor_y = self.locked_cursor_y.clone();
            
            std::thread::spawn(move || {
                println!("[INPUT-THREAD] 🚀 Dedicated RAW INPUT processing thread started");
                println!("[INPUT-THREAD] 🎯 Activated by GPUI right-click, deactivated by GPUI release");
                let device_state = DeviceState::new();
                let mut last_mouse_pos: Option<(i32, i32)> = None;
                
                loop {
                    // Mark when we start processing input
                    let input_start = std::time::Instant::now();

                    // Sleep for ~8ms (~120Hz processing rate)
                    std::thread::sleep(std::time::Duration::from_millis(8));

                    // Check if camera controls are active (set by GPUI mouse events)
                    let is_rotating = mouse_right_captured.load(Ordering::Acquire);
                    let is_panning = mouse_middle_captured.load(Ordering::Acquire);

                    if !is_rotating && !is_panning {
                        // Not active - clear ALL state immediately
                        last_mouse_pos = None;

                        // Clear atomic input state
                        input_state_for_thread.forward.store(0, Ordering::Relaxed);
                        input_state_for_thread.right.store(0, Ordering::Relaxed);
                        input_state_for_thread.up.store(0, Ordering::Relaxed);
                        input_state_for_thread.boost.store(false, Ordering::Relaxed);

                        // CRITICAL: Also push zeros to GPU to stop camera immediately
                        if let Ok(engine) = gpu_engine_for_thread.try_lock() {
                            if let Some(ref bevy_renderer) = engine.bevy_renderer {
                                if let Ok(mut input) = bevy_renderer.camera_input.try_lock() {
                                    input.forward = 0.0;
                                    input.right = 0.0;
                                    input.up = 0.0;
                                    input.boost = false;
                                    input.mouse_delta_x = 0.0;
                                    input.mouse_delta_y = 0.0;
                                    input.pan_delta_x = 0.0;
                                    input.pan_delta_y = 0.0;
                                    input.zoom_delta = 0.0;
                                }
                            }
                        }

                        continue; // Wait for activation
                    }

                    // Camera controls are ACTIVE - poll input at high frequency
                    // Poll keyboard for WASD
                    let keys: Vec<Keycode> = device_state.get_keys();

                    let forward = if keys.contains(&Keycode::W) { 1 } else if keys.contains(&Keycode::S) { -1 } else { 0 };
                    let right = if keys.contains(&Keycode::D) { 1 } else if keys.contains(&Keycode::A) { -1 } else { 0 };
                    let up = if keys.contains(&Keycode::E) || keys.contains(&Keycode::Space) { 1 }
                            else if keys.contains(&Keycode::Q) || keys.contains(&Keycode::LShift) || keys.contains(&Keycode::RShift) { -1 }
                            else { 0 };
                    let boost = keys.contains(&Keycode::LShift) || keys.contains(&Keycode::RShift);

                    input_state_for_thread.forward.store(forward, Ordering::Relaxed);
                    input_state_for_thread.right.store(right, Ordering::Relaxed);
                    input_state_for_thread.up.store(up, Ordering::Relaxed);
                    input_state_for_thread.boost.store(boost, Ordering::Relaxed);

                    // WINDOWS: Read cursor position directly and reset it (standard game approach)
                    #[cfg(target_os = "windows")]
                    {
                        let locked_screen_x = locked_cursor_x.load(Ordering::Relaxed);
                        let locked_screen_y = locked_cursor_y.load(Ordering::Relaxed);

                        if locked_screen_x != 0 || locked_screen_y != 0 {
                            unsafe {
                                let mut point = POINT { x: 0, y: 0 };
                                if GetCursorPos(&mut point) != 0 {
                                    // Calculate delta from locked position
                                    let dx = point.x - locked_screen_x;
                                    let dy = point.y - locked_screen_y;

                                    if dx != 0 || dy != 0 {
                                        // Accumulate deltas (convert to * 1000 for precision)
                                        if is_rotating {
                                            input_state_for_thread.mouse_delta_x.fetch_add(dx * 1000, Ordering::Relaxed);
                                            input_state_for_thread.mouse_delta_y.fetch_add(dy * 1000, Ordering::Relaxed);
                                        }
                                        if is_panning {
                                            input_state_for_thread.pan_delta_x.fetch_add(dx * 1000, Ordering::Relaxed);
                                            input_state_for_thread.pan_delta_y.fetch_add(dy * 1000, Ordering::Relaxed);
                                        }

                                        // Reset cursor to locked position (standard game technique)
                                        SetCursorPos(locked_screen_x, locked_screen_y);
                                    }
                                }
                            }
                        }
                    }

                    // NON-WINDOWS: Use GPUI mouse events (fallback)
                    #[cfg(not(target_os = "windows"))]
                    {
                        let current_x = input_state_for_thread.mouse_x.load(Ordering::Relaxed);
                        let current_y = input_state_for_thread.mouse_y.load(Ordering::Relaxed);

                        if let Some((last_x, last_y)) = last_mouse_pos {
                            let dx = current_x - last_x;
                            let dy = current_y - last_y;

                            if dx != 0 || dy != 0 {
                                if is_rotating {
                                    input_state_for_thread.mouse_delta_x.fetch_add(dx, Ordering::Relaxed);
                                    input_state_for_thread.mouse_delta_y.fetch_add(dy, Ordering::Relaxed);
                                }
                                if is_panning {
                                    input_state_for_thread.pan_delta_x.fetch_add(dx, Ordering::Relaxed);
                                    input_state_for_thread.pan_delta_y.fetch_add(dy, Ordering::Relaxed);
                                }
                                last_mouse_pos = Some((current_x, current_y));
                            }
                        } else {
                            last_mouse_pos = Some((current_x, current_y));
                        }
                    }
                    
                    // Try to push to GPU (non-blocking)
                    if let Ok(engine) = gpu_engine_for_thread.try_lock() {
                        if let Some(ref bevy_renderer) = engine.bevy_renderer {
                            if let Ok(mut input) = bevy_renderer.camera_input.try_lock() {
                                // Read atomic values
                                input.forward = input_state_for_thread.forward.load(Ordering::Relaxed) as f32;
                                input.right = input_state_for_thread.right.load(Ordering::Relaxed) as f32;
                                input.up = input_state_for_thread.up.load(Ordering::Relaxed) as f32;
                                input.boost = input_state_for_thread.boost.load(Ordering::Relaxed);
                                
                                // Read and CLEAR deltas
                                let mouse_x = input_state_for_thread.mouse_delta_x.swap(0, Ordering::Relaxed);
                                let mouse_y = input_state_for_thread.mouse_delta_y.swap(0, Ordering::Relaxed);
                                input.mouse_delta_x = mouse_x as f32 / 1000.0;
                                input.mouse_delta_y = mouse_y as f32 / 1000.0;
                                
                                let pan_x = input_state_for_thread.pan_delta_x.swap(0, Ordering::Relaxed);
                                let pan_y = input_state_for_thread.pan_delta_y.swap(0, Ordering::Relaxed);
                                input.pan_delta_x = pan_x as f32 / 1000.0;
                                input.pan_delta_y = pan_y as f32 / 1000.0;
                                
                                let zoom = input_state_for_thread.zoom_delta.swap(0, Ordering::Relaxed);
                                input.zoom_delta = zoom as f32 / 1000.0;
                                
                                input.move_speed = input_state_for_thread.move_speed.load(Ordering::Relaxed) as f32 / 100.0;
                                
                                // Track latency
                                let input_latency = input_start.elapsed().as_micros() as u64;
                                input_state_for_thread.input_latency_us.store(input_latency, Ordering::Relaxed);
                            }
                        }
                    }
                }
            });
        }

        // Clone for scroll wheel handler
        let input_state_scroll = self.input_state.clone();
        
        // Clone for left-click object selection
        let gpu_engine_for_click = gpu_engine.clone();
        
        // Clone element bounds for tracking
        let element_bounds_for_prepaint = self.element_bounds.clone();
        let element_bounds_for_click = self.element_bounds.clone();
        
        // Clone gpu_engine for viewport bounds update
        let gpu_engine_clone = gpu_engine.clone();
        
        let mut viewport_div = div()
            .flex() // Enable flexbox
            .flex_col() // Column direction
            .flex_1() // Allow growing to fill parent
            .size_full() // Take full size
            .relative()
            // TRANSPARENT - no background! This creates the "hole" to see winit's green bg
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            // CRITICAL: Capture element bounds for accurate coordinate conversion
            .on_children_prepainted(move |children_bounds: Vec<Bounds<Pixels>>, _window, _cx| {
                if !children_bounds.is_empty() {
                    // Calculate bounding box from all children (window coordinates)
                    let mut min_x = f32::MAX;
                    let mut min_y = f32::MAX;
                    let mut max_x = f32::MIN;
                    let mut max_y = f32::MIN;
                    
                    for bounds in &children_bounds {
                        let bounds_min_x: f32 = bounds.origin.x.into();
                        let bounds_min_y: f32 = bounds.origin.y.into();
                        let bounds_width: f32 = bounds.size.width.into();
                        let bounds_height: f32 = bounds.size.height.into();
                        
                        min_x = min_x.min(bounds_min_x);
                        min_y = min_y.min(bounds_min_y);
                        max_x = max_x.max(bounds_min_x + bounds_width);
                        max_y = max_y.max(bounds_min_y + bounds_height);
                    }
                    
                    let bounds = Bounds {
                        origin: point(px(min_x), px(min_y)),
                        size: size(px(max_x - min_x), px(max_y - min_y)),
                    };
                    
                    *element_bounds_for_prepaint.borrow_mut() = Some(bounds);
                    
                    // Update Bevy camera viewport to match GPUI viewport bounds
                    // This ensures Bevy renders exactly to the visible region
                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                        if let Some(ref bevy_renderer) = engine.bevy_renderer {
                            if let Ok(mut camera_input) = bevy_renderer.camera_input.try_lock() {
                                camera_input.viewport_x = min_x;
                                camera_input.viewport_y = min_y;
                                camera_input.viewport_width = max_x - min_x;
                                camera_input.viewport_height = max_y - min_y;
                            }
                        }
                    }
                    
                    // Debug log (only occasionally to avoid spam)
                    static mut FRAME_COUNT: u32 = 0;
                    unsafe {
                        FRAME_COUNT += 1;
                    }
                }
            })
            // Track mouse movement to update atomic position state (for non-Windows platforms)
            .on_mouse_move({
                let input_state_clone = self.input_state.clone();
                let mouse_right_captured = self.mouse_right_captured.clone();
                let mouse_middle_captured = self.mouse_middle_captured.clone();

                move |event, window, _cx| {
                    // Check if camera controls are active
                    let is_rotating = mouse_right_captured.load(Ordering::Acquire);
                    let is_panning = mouse_middle_captured.load(Ordering::Acquire);

                    if is_rotating || is_panning {
                        // Keep cursor hidden during camera controls (GPUI might try to show it)
                        #[cfg(target_os = "windows")]
                        {
                            // Ensure cursor stays hidden on Windows
                            // Note: We don't call hide_cursor() repeatedly as it's counter-based
                            // Just make sure GPUI doesn't override our hidden state
                        }

                        #[cfg(not(target_os = "windows"))]
                        {
                            // Update position for input thread to calculate deltas
                            let x = (event.position.x.as_f32() * 1000.0) as i32;
                            let y = (event.position.y.as_f32() * 1000.0) as i32;
                            input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                            input_state_clone.mouse_y.store(y, Ordering::Relaxed);
                        }

                        // Keep GPUI cursor style hidden as well
                        window.set_window_cursor_style(CursorStyle::None);
                    } else {
                        // Not rotating/panning - update position normally for UI interactions
                        let x = (event.position.x.as_f32() * 1000.0) as i32;
                        let y = (event.position.y.as_f32() * 1000.0) as i32;
                        input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                        input_state_clone.mouse_y.store(y, Ordering::Relaxed);
                    }
                }
            })
            // Right-click DOWN on viewport = ACTIVATE camera controls
            .on_mouse_down(gpui::MouseButton::Right, {
                let mouse_right_captured = self.mouse_right_captured.clone();
                let mouse_middle_captured = self.mouse_middle_captured.clone();
                let locked_cursor_x = self.locked_cursor_x.clone();
                let locked_cursor_y = self.locked_cursor_y.clone();
                let locked_cursor_screen_x = self.locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = self.locked_cursor_screen_y.clone();
                let input_state_clone = self.input_state.clone();
                move |event, window, _cx| {
                    println!("[VIEWPORT] 🖱️ Right-click DOWN on viewport - ACTIVATING camera controls");

                    // Check if Shift is held for pan mode
                    let shift_pressed = event.modifiers.shift;

                    // Get cursor position from event
                    let window_x = event.position.x.as_f32();
                    let window_y = event.position.y.as_f32();

                    // Convert to screen coordinates for cursor locking
                    if let Some((screen_x, screen_y)) = window_to_screen_position(window, window_x, window_y) {
                        // Store screen coordinates for input thread (Windows uses these directly)
                        locked_cursor_x.store(screen_x, Ordering::Relaxed);
                        locked_cursor_y.store(screen_y, Ordering::Relaxed);
                        locked_cursor_screen_x.store(screen_x, Ordering::Relaxed);
                        locked_cursor_screen_y.store(screen_y, Ordering::Relaxed);
                        println!("[VIEWPORT] 📍 Locked cursor at screen position ({}, {})", screen_x, screen_y);

                        // Confine cursor to window bounds (less aggressive than point lock)
                        lock_cursor_to_window(window);
                    } else {
                        println!("[VIEWPORT] ⚠️ Failed to convert window position to screen position");
                        // Fallback: lock to window bounds
                        lock_cursor_to_window(window);
                    }

                    // Store window-relative position for non-Windows platforms
                    let x = (window_x * 1000.0) as i32;
                    let y = (window_y * 1000.0) as i32;
                    input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                    input_state_clone.mouse_y.store(y, Ordering::Relaxed);

                    if shift_pressed {
                        // Shift + Right = Pan mode
                        mouse_middle_captured.store(true, Ordering::Release);
                        println!("[VIEWPORT] 🎥 Pan mode activated (Shift + Right)");
                    } else {
                        // Right alone = Rotate mode
                        mouse_right_captured.store(true, Ordering::Release);
                        println!("[VIEWPORT] 🎥 Rotate mode activated (Right)");
                    }

                    // Hide cursor using proper Windows API
                    hide_cursor();
                    window.set_window_cursor_style(CursorStyle::None); // Also set GPUI style for fallback
                }
            })
            // Right-click UP anywhere = DEACTIVATE camera controls
            .on_mouse_up(gpui::MouseButton::Right, {
                let mouse_right_captured = self.mouse_right_captured.clone();
                let mouse_middle_captured = self.mouse_middle_captured.clone();
                let locked_cursor_screen_x = self.locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = self.locked_cursor_screen_y.clone();
                move |_event, window, _cx| {
                    println!("[VIEWPORT] 🖱️ Right-click UP - DEACTIVATING camera controls");

                    // Deactivate both modes
                    mouse_right_captured.store(false, Ordering::Release);
                    mouse_middle_captured.store(false, Ordering::Release);

                    // Clear locked screen position
                    locked_cursor_screen_x.store(0, Ordering::Relaxed);
                    locked_cursor_screen_y.store(0, Ordering::Relaxed);

                    // Restore cursor visibility using proper Windows API
                    show_cursor();
                    window.set_window_cursor_style(CursorStyle::Arrow); // Also set GPUI style
                    unlock_cursor();

                    println!("[VIEWPORT] ✅ Camera controls deactivated, cursor restored");
                }
            })
            // Left-click for object selection in edit mode
            .on_mouse_down(gpui::MouseButton::Left, {
                let gpu_engine_click = gpu_engine_for_click.clone();
                let element_bounds = element_bounds_for_click.clone();
                move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, _cx: &mut gpui::App| {
                    println!("[VIEWPORT] 🖱️ Left-click detected at window position: {:?}", event.position);
                    
                    // Convert window coordinates to element-relative coordinates
                    let bounds_opt = element_bounds.borrow();
                    let (element_x, element_y, viewport_width, viewport_height) = if let Some(ref bounds) = *bounds_opt {
                        let origin_x: f32 = bounds.origin.x.into();
                        let origin_y: f32 = bounds.origin.y.into();
                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();
                        
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        
                        // Convert to element-relative coordinates
                        let elem_x = pos_x - origin_x;
                        let elem_y = pos_y - origin_y;
                        
                        println!("[VIEWPORT] 📐 Element-relative position: ({:.1}, {:.1}) in viewport ({:.1}x{:.1})", 
                            elem_x, elem_y, width, height);
                        
                        (elem_x, elem_y, width, height)
                    } else {
                        // Fallback: use window coordinates (first frame before bounds captured)
                        println!("[VIEWPORT] ⚠️ Element bounds not yet captured, using window coords");
                        let window_size = window.viewport_size();
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        let width: f32 = window_size.width.into();
                        let height: f32 = window_size.height.into();
                        (pos_x, pos_y, width, height)
                    };
                    
                    // Convert to normalized coordinates (0.0 to 1.0)
                    let normalized_x = (element_x / viewport_width).clamp(0.0, 1.0);
                    let normalized_y = (element_y / viewport_height).clamp(0.0, 1.0);
                    
                    println!("[VIEWPORT] 🎯 Normalized position: ({:.3}, {:.3})", normalized_x, normalized_y);
                    
                    // Send to Bevy's ViewportMouseInput via shared resource
                    if let Ok(engine) = gpu_engine_click.try_lock() {
                        if let Some(ref bevy_renderer) = engine.bevy_renderer {
                            // Update shared viewport mouse input
                            let mut mouse_input = bevy_renderer.viewport_mouse_input.lock();
                            // Use engine_backend's Vec2 type instead of bevy::prelude::Vec2
                            mouse_input.mouse_pos.x = normalized_x;
                            mouse_input.mouse_pos.y = normalized_y;
                            mouse_input.left_clicked = true;
                            mouse_input.left_down = true;
                            
                            println!("[VIEWPORT] ✅ Sent click to Bevy (will be processed by raycast system)");
                        } else {
                            println!("[VIEWPORT] ⚠️ Bevy renderer not available");
                        }
                    } else {
                        println!("[VIEWPORT] ⚠️ Could not lock GPU engine for click event");
                    }
                }
            })
            // Track mouse movement for gizmo dragging (while left button is down)
            .on_mouse_move({
                let gpu_engine_move = gpu_engine_for_click.clone();
                let element_bounds_move = element_bounds_for_click.clone();
                let last_mouse_pos = Rc::new(RefCell::new(Option::<(f32, f32)>::None));
                
                move |event: &gpui::MouseMoveEvent, _window, _cx| {
                    // Convert window to element coordinates
                    let bounds_opt = element_bounds_move.borrow();
                    let (element_x, element_y, viewport_width, viewport_height) = if let Some(ref bounds) = *bounds_opt {
                        let origin_x: f32 = bounds.origin.x.into();
                        let origin_y: f32 = bounds.origin.y.into();
                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        (pos_x - origin_x, pos_y - origin_y, width, height)
                    } else {
                        return; // Skip if bounds not captured yet
                    };
                    
                    let normalized_x = (element_x / viewport_width).clamp(0.0, 1.0);
                    let normalized_y = (element_y / viewport_height).clamp(0.0, 1.0);
                    
                    // Calculate delta from last position
                    let mut last_pos = last_mouse_pos.borrow_mut();
                    let (delta_x, delta_y) = if let Some((last_x, last_y)) = *last_pos {
                        (normalized_x - last_x, normalized_y - last_y)
                    } else {
                        (0.0, 0.0)
                    };
                    
                    *last_pos = Some((normalized_x, normalized_y));
                    drop(last_pos); // Release borrow
                    
                    // Update Bevy mouse input
                    if let Ok(engine) = gpu_engine_move.try_lock() {
                        if let Some(ref bevy_renderer) = engine.bevy_renderer {
                            let mut mouse_input = bevy_renderer.viewport_mouse_input.lock();
                            mouse_input.mouse_pos.x = normalized_x;
                            mouse_input.mouse_pos.y = normalized_y;
                            mouse_input.mouse_delta.x = delta_x;
                            mouse_input.mouse_delta.y = delta_y;
                        }
                    }
                }
            })
            // Clear left_clicked flag on mouse up (so it's only true for one frame)
            .on_mouse_up(gpui::MouseButton::Left, {
                let gpu_engine_up = gpu_engine_for_click.clone();
                move |_event: &gpui::MouseUpEvent, _window: &mut gpui::Window, _cx: &mut gpui::App| {
                    if let Ok(engine) = gpu_engine_up.try_lock() {
                        if let Some(ref bevy_renderer) = engine.bevy_renderer {
                            let mut mouse_input = bevy_renderer.viewport_mouse_input.lock();
                            mouse_input.left_clicked = false; // Only true for one frame
                            mouse_input.left_down = false;
                        }
                    }
                }
            })
            .child(
                // Main viewport - input thread handles ALL mouse/keyboard when focused
                div()
                    .flex() // Enable flex
                    .flex_1() // Grow to fill available space
                    .size_full() // Take full size
                    .on_scroll_wheel(move |event: &gpui::ScrollWheelEvent, _phase, _cx| {
                        let scroll_delta: f32 = event.delta.pixel_delta(px(1.0)).y.into();
                        
                        // Always zoom - simpler behavior
                        input_state_scroll.set_zoom_delta(scroll_delta * 0.5);
                    })
                    .child(self.viewport.clone())
            )
            .when(state.show_camera_mode_selector, |viewport_div| {
                let state_arc_clone = state_arc.clone();
                viewport_div.child(
                    // Camera mode selector (bottom-left)
                    div()
                        .absolute()
                        .bottom_4()
                        .left_4()
                        .child(Self::render_camera_mode_selector(state.camera_mode, state.camera_mode_selector_collapsed, state_arc_clone, cx))
                )
            })
            .when(state.show_viewport_options, |viewport_div| {
                let state_arc_clone = state_arc.clone();
                viewport_div.child(
                    // Grid and rendering options (top-left)
                    div()
                        .absolute()
                        .top_4()
                        .left_4()
                        .child(Self::render_viewport_options(state, state_arc_clone, gpu_engine, cx))
                )
            });

        // Performance overlays in bottom-right corner
        if state.show_performance_overlay {
            let state_arc_clone = state_arc.clone();
            let state_arc_clone_gpu = state_arc.clone();
            
            // GPU Pipeline Stats - positioned to the left of performance overlay
            viewport_div = viewport_div.child(
                div()
                    .absolute()
                    .bottom_4()
                    .right(px(380.0)) // Position to the left of performance overlay (360px width + 20px gap)
                    .child(self.render_gpu_pipeline_overlay(state, state_arc_clone_gpu, gpu_engine, cx))
            );
            
            // Performance overlay - bottom-right corner
            let mut perf_container = div()
                .id("performance-overlay-container")
                .absolute()
                .bottom_4()
                .right_4();
            
            // Only add width/height constraints when expanded
            if !state.performance_overlay_collapsed {
                perf_container = perf_container
                    .w(px(360.0)) // Expanded width for graph
                    .max_h(px(700.0)) // Max height before scrolling
                    .overflow_y_scroll(); // Enable vertical scrolling
            }
            
            viewport_div = viewport_div.child(
                perf_container.child(self.render_performance_overlay(state, state_arc_clone, fps_graph_state, gpu_engine, game_thread, cx))
            );
        }

        viewport_div
    }

    fn render_camera_mode_selector<V: 'static>(
        camera_mode: CameraMode,
        collapsed: bool,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        if collapsed {
            // Collapsed state - just show a button to expand
            Button::new("expand_camera_mode")
                .icon(IconName::Cube)
                .tooltip("Camera Modes")
                .on_click(move |_, _, _| {
                    state_arc.write().set_camera_mode_selector_collapsed(false);
                })
                .into_any_element()
        } else {
            // Expanded state - show full toolbar
            h_flex()
                .gap_1()
                .p_1()
                .bg(cx.theme().background.opacity(0.9))
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .justify_between()
                .items_center()
                .child(
                    h_flex()
                        .gap_1()
                        .child({
                            let state_clone = state_arc.clone();
                            Button::new("camera_perspective")
                                .icon(IconName::Cube)
                                .tooltip("Perspective View")
                                .selected(matches!(camera_mode, CameraMode::Perspective))
                                .on_click(move |_, _, _| {
                                    state_clone.write().set_camera_mode(CameraMode::Perspective);
                                })
                        })
                        .child({
                            let state_clone = state_arc.clone();
                            Button::new("camera_orthographic")
                                .icon(IconName::Square)
                                .tooltip("Orthographic View")
                                .selected(matches!(camera_mode, CameraMode::Orthographic))
                                .on_click(move |_, _, _| {
                                    state_clone.write().set_camera_mode(CameraMode::Orthographic);
                                })
                        })
                        .child({
                            let state_clone = state_arc.clone();
                            Button::new("camera_top")
                                .icon(IconName::ArrowUp)
                                .tooltip("Top View")
                                .selected(matches!(camera_mode, CameraMode::Top))
                                .on_click(move |_, _, _| {
                                    state_clone.write().set_camera_mode(CameraMode::Top);
                                })
                        })
                        .child({
                            let state_clone = state_arc.clone();
                            Button::new("camera_front")
                                .icon(IconName::ArrowRight)
                                .tooltip("Front View")
                                .selected(matches!(camera_mode, CameraMode::Front))
                                .on_click(move |_, _, _| {
                                    state_clone.write().set_camera_mode(CameraMode::Front);
                                })
                        })
                        .child({
                            let state_clone = state_arc.clone();
                            Button::new("camera_side")
                                .icon(IconName::ArrowLeft)
                                .tooltip("Side View")
                                .selected(matches!(camera_mode, CameraMode::Side))
                                .on_click(move |_, _, _| {
                                    state_clone.write().set_camera_mode(CameraMode::Side);
                                })
                        })
                )
                .child({
                    let state_clone = state_arc.clone();
                    Button::new("collapse_camera_mode")
                        .icon(IconName::X)
                        .ghost()
                        .on_click(move |_, _, _| {
                            state_clone.write().set_camera_mode_selector_collapsed(true);
                        })
                })
                .into_any_element()
        }
    }

    fn render_viewport_options<V: 'static>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        if state.viewport_options_collapsed {
            // Collapsed state - just show a button to expand
            Button::new("expand_viewport_options")
                .icon(IconName::LayoutDashboard)
                .tooltip("Viewport Options")
                .on_click(move |_, _, _| {
                    state_arc.write().set_viewport_options_collapsed(false);
                })
                .into_any_element()
        } else {
            // Expanded state - show full toolbar with view modes
            v_flex()
                .gap_1()
                .p_1()
                .bg(cx.theme().background.opacity(0.9))
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                // First row: View mode and debug visualization toggles
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child({
                            let gpu_engine_clone = gpu_engine.clone();
                            let state_clone = state_arc.clone();
                            Button::new("view_lit")
                                .label("Lit")
                                .tooltip("Full lighting and textures")
                                .selected(matches!(state.current_view_mode, engine_backend::subsystems::render::ViewMode::Lit))
                                .small()
                                .on_click(move |_, _, _| {
                                    state_clone.write().current_view_mode = engine_backend::subsystems::render::ViewMode::Lit;
                                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                                        engine.set_view_mode(engine_backend::subsystems::render::ViewMode::Lit);
                                    }
                                })
                        })
                        .child({
                            let gpu_engine_clone = gpu_engine.clone();
                            let state_clone = state_arc.clone();
                            Button::new("view_unlit")
                                .label("Unlit")
                                .tooltip("No lighting, base colors only")
                                .selected(matches!(state.current_view_mode, engine_backend::subsystems::render::ViewMode::Unlit))
                                .small()
                                .on_click(move |_, _, _| {
                                    state_clone.write().current_view_mode = engine_backend::subsystems::render::ViewMode::Unlit;
                                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                                        engine.set_view_mode(engine_backend::subsystems::render::ViewMode::Unlit);
                                    }
                                })
                        })
                        .child({
                            let gpu_engine_clone = gpu_engine.clone();
                            let state_clone = state_arc.clone();
                            Button::new("view_wireframe_only")
                                .label("Wire")
                                .tooltip("Wireframe only")
                                .selected(matches!(state.current_view_mode, engine_backend::subsystems::render::ViewMode::Wireframe))
                                .small()
                                .on_click(move |_, _, _| {
                                    state_clone.write().current_view_mode = engine_backend::subsystems::render::ViewMode::Wireframe;
                                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                                        engine.set_view_mode(engine_backend::subsystems::render::ViewMode::Wireframe);
                                    }
                                })
                        })
                        .child({
                            let gpu_engine_clone = gpu_engine.clone();
                            let state_clone = state_arc.clone();
                            Button::new("debug_shader")
                                .label("Shader")
                                .tooltip("Shader complexity heatmap")
                                .selected(matches!(state.current_debug_viz, engine_backend::subsystems::render::DebugVisualization::ShaderComplexity))
                                .small()
                                .on_click(move |_, _, _| {
                                    let new_viz = if matches!(state_clone.read().current_debug_viz, engine_backend::subsystems::render::DebugVisualization::ShaderComplexity) {
                                        engine_backend::subsystems::render::DebugVisualization::None
                                    } else {
                                        engine_backend::subsystems::render::DebugVisualization::ShaderComplexity
                                    };
                                    state_clone.write().current_debug_viz = new_viz;
                                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                                        engine.set_debug_visualization(new_viz);
                                    }
                                })
                        })
                        .child({
                            let gpu_engine_clone = gpu_engine.clone();
                            let state_clone = state_arc.clone();
                            Button::new("debug_light")
                                .label("Lighting")
                                .tooltip("Light complexity (lights per pixel)")
                                .selected(matches!(state.current_debug_viz, engine_backend::subsystems::render::DebugVisualization::LightComplexity))
                                .small()
                                .on_click(move |_, _, _| {
                                    let new_viz = if matches!(state_clone.read().current_debug_viz, engine_backend::subsystems::render::DebugVisualization::LightComplexity) {
                                        engine_backend::subsystems::render::DebugVisualization::None
                                    } else {
                                        engine_backend::subsystems::render::DebugVisualization::LightComplexity
                                    };
                                    state_clone.write().current_debug_viz = new_viz;
                                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                                        engine.set_debug_visualization(new_viz);
                                    }
                                })
                        })
                )
                // Second row: Toggle buttons for grid, wireframe overlay, and lighting
                .child(
                    h_flex()
                        .gap_1()
                        .justify_between()
                        .items_center()
                        .child(
                            h_flex()
                                .gap_1()
                                .child({
                                    let state_clone = state_arc.clone();
                                    let gpu_engine_clone = gpu_engine.clone();
                                    Button::new("toggle_grid")
                                        .icon(IconName::LayoutDashboard)
                                        .tooltip("Toggle Grid")
                                        .selected(state.show_grid)
                                        .on_click(move |_, _, _| {
                                            state_clone.write().toggle_grid();
                                            // Update Bevy renderer
                                            if let Ok(engine) = gpu_engine_clone.try_lock() {
                                                engine.toggle_grid();
                                            }
                                        })
                                })
                                .child({
                                    let state_clone = state_arc.clone();
                                    let gpu_engine_clone = gpu_engine.clone();
                                    Button::new("toggle_wireframe")
                                        .icon(IconName::Triangle)
                                        .tooltip("Toggle Wireframe")
                                        .selected(state.show_wireframe)
                                        .on_click(move |_, _, _| {
                                            state_clone.write().toggle_wireframe();
                                            // Update Bevy renderer
                                            if let Ok(engine) = gpu_engine_clone.try_lock() {
                                                engine.toggle_wireframe();
                                            }
                                        })
                                })
                                .child({
                                    let state_clone = state_arc.clone();
                                    let gpu_engine_clone = gpu_engine.clone();
                                    Button::new("toggle_lighting")
                                        .icon(IconName::Sun)
                                        .tooltip("Toggle Lighting")
                                        .selected(state.show_lighting)
                                        .on_click(move |_, _, _| {
                                            state_clone.write().toggle_lighting();
                                            // Update Bevy renderer
                                            if let Ok(engine) = gpu_engine_clone.try_lock() {
                                                engine.toggle_lighting();
                                            }
                                        })
                                })
                        )
                        .child({
                            let state_clone = state_arc.clone();
                            Button::new("collapse_viewport_options")
                                .icon(IconName::X)
                                .ghost()
                                .on_click(move |_, _, _| {
                                    state_clone.write().set_viewport_options_collapsed(true);
                                })
                        })
                )
                .into_any_element()
        }
    }

    fn render_viewport_controls_overlay<V: 'static>(cx: &mut Context<V>) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        h_flex()
            .gap_2()
            .p_2()
            .w_full()
            .bg(cx.theme().background.opacity(0.9))
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .justify_between()
            .items_center()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("grid")
                            .icon(IconName::LayoutDashboard)
                            .tooltip("Toggle Grid")
                            
                    )
                    .child(
                        Button::new("axes")
                            .icon(IconName::Axes)
                            .tooltip("Toggle Axes")
                            
                    )
                    .child(
                        Button::new("perspective")
                            .icon(IconName::Cube)
                            .tooltip("Toggle Perspective")
                            
                    )
            )
            .child(
                Button::new("close_viewport_controls")
                    .icon(IconName::X)
                    .ghost()
                    
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.dispatch_action(&ToggleViewportOptions);
                    }))
            )
    }

    /// Render GPU Pipeline Stats Overlay - Like Unreal's "stat gpu"
    /// Shows REAL measured timings for each render pass
    fn render_gpu_pipeline_overlay<V: 'static>(
        &mut self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        if state.gpu_pipeline_overlay_collapsed {
            // Collapsed state - just show a button to expand
            return Button::new("expand_gpu_pipeline")
                .icon(IconName::Activity)
                .tooltip("GPU Pipeline Stats")
                .on_click(move |_, _, _| {
                    state_arc.write().set_gpu_pipeline_overlay_collapsed(false);
                })
                .into_any_element();
        }
        
        // Expanded state - show full overlay
        // Get GPU profiler data
        let gpu_data = if let Ok(engine) = gpu_engine.lock() {
            engine.get_gpu_profiler_data()
        } else {
            None
        };

        // Get theme data and clone colors to avoid borrowing issues
        let (background, radius, border, foreground, chart_1, chart_2, chart_3, chart_4, chart_5, warning, success) = {
            let theme = cx.theme();
            (theme.background, theme.radius, theme.border, theme.foreground, theme.chart_1, theme.chart_2, theme.chart_3, theme.chart_4, theme.chart_5, theme.warning, theme.success)
        };

        // Collect pass data from dynamic render metrics
        let pass_data = if let Some(ref data) = gpu_data {
            // Filter out non-render metrics (fps, frame_time, frame_count)
            let mut render_passes: Vec<_> = data.render_metrics.iter()
                .filter(|metric| metric.path.starts_with("render/") && metric.value_ms > 0.0)
                .collect();
            
            // Sort by value descending to show most expensive passes first
            render_passes.sort_by(|a, b| b.value_ms.partial_cmp(&a.value_ms).unwrap_or(std::cmp::Ordering::Equal));
            
            // Create pass data with cycling colors
            let colors = [chart_1, chart_2, chart_3, chart_4, chart_5, warning, success];
            render_passes.into_iter()
                .take(15) // Limit to 15 most expensive passes to avoid UI overflow
                .enumerate()
                .map(|(i, metric)| {
                    let color = colors[i % colors.len()];
                    (metric.name.clone(), metric.value_ms, metric.percentage, color)
                })
                .collect()
        } else {
            vec![]
        };
        
        let mut result = v_flex()
            .gap_1()
            .p_2()
            .w_full()
            .bg(background.opacity(0.95))
            .rounded(radius)
            .border_1()
            .border_color(border)
            .child(
                // Header
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(foreground)
                            .child("🔥 GPU Pipeline Stats")
                    )
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("collapse_gpu_pipeline")
                            .icon(IconName::X)
                            .ghost()
                            .on_click(move |_, _, _| {
                                state_clone.write().set_gpu_pipeline_overlay_collapsed(true);
                            })
                    })
            )
            .child(
                // Total frame time
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Frame Time:")
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(foreground)
                            .child(if let Some(ref data) = gpu_data {
                                format!("{:.2}ms ({:.0} FPS)", data.total_frame_ms, data.fps)
                            } else {
                                "N/A".to_string()
                            })
                    )
            )
            .child(
                div()
                    .w_full()
                    .h(px(1.0))
                    .bg(border)
            );
        
        // Add individual pass timings - create elements one by one to avoid borrow checker issues
        for (name, time_ms, percent, color) in pass_data {
            let elem = Self::render_pass_stat_elem(name, time_ms, percent, color, cx);
            result = result.child(elem);
        }
        
        // Add separator and total
        if let Some(ref data) = gpu_data {
            result = result
                .child(
                    div()
                        .w_full()
                        .h(px(1.0))
                        .bg(border)
                        .mt_1()
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(foreground)
                                .child("Total GPU:")
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(if data.total_gpu_ms < 8.0 {
                                    success
                                } else if data.total_gpu_ms < 16.0 {
                                    warning
                                } else {
                                    cx.theme().danger
                                })
                                .child(format!("{:.2}ms", data.total_gpu_ms))
                        )
                );
        }
        
        result.into_any_element()
    }

    /// Render a single GPU pass stat line with timing and percentage
    fn render_pass_stat_elem<V: 'static>(
        name: String,
        time_ms: f32,
        percent: f32,
        color: Hsla,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .child(
                // Color indicator
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded(px(2.0))
                    .bg(color)
            )
            .child(
                // Name
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(name)
            )
            .child(
                // Timing
                div()
                    .text_xs()
                    .text_color(theme.foreground)
                    .child(format!("{:.2}ms", time_ms))
            )
            .child(
                // Percentage
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("({:.1}%)", percent))
            )
    }

    fn render_performance_overlay<V: 'static>(
        &mut self,
        state: &mut LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        fps_graph_state: Rc<RefCell<bool>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        game_thread: &Arc<GameThread>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        if state.performance_overlay_collapsed {
            // Collapsed state - just show a button to expand
            return Button::new("expand_performance")
                .icon(IconName::ChartPie)
                .tooltip("Performance Overlay")
                .on_click(move |_, _, _| {
                    state_arc.write().set_performance_overlay_collapsed(false);
                })
                .into_any_element();
        }
        
        // Expanded state - show full overlay
        // Get comprehensive metrics
        let (ui_fps, bevy_fps, pipeline_us, gpu_us, cpu_us, render_metrics) = if let Ok(engine) = gpu_engine.lock() {
            let ui_fps = engine.get_fps();
            let bevy_fps = engine.get_bevy_fps();
            let pipeline = engine.get_pipeline_time_us();
            let gpu = engine.get_gpu_time_us();
            let cpu = engine.get_cpu_time_us();
            let metrics = engine.get_render_metrics();
            (ui_fps, bevy_fps, pipeline, gpu, cpu, metrics)
        } else {
            (0.0, 0.0, 0, 0, 0, None)
        };

        // Extract additional metrics
        let (memory_mb, draw_calls, vertices_drawn) = if let Some(ref metrics) = render_metrics {
            (metrics.memory_usage_mb, metrics.draw_calls, metrics.vertices_drawn)
        } else {
            (0.0, 0, 0)
        };

        // Get game thread metrics
        let game_tps = game_thread.get_tps();
        let game_tick_count = game_thread.get_tick_count();
        let game_enabled = game_thread.is_enabled();

        // Update FPS history - track BOTH UI and Bevy FPS
        let mut fps_history = self.fps_history.borrow_mut();
        let mut fps_sample_counter = self.fps_sample_counter.borrow_mut();
        
        fps_history.push_back(FpsDataPoint {
            index: *fps_sample_counter,
            fps: ui_fps as f64,
        });
        *fps_sample_counter += 1;
        
        // Keep only last 120 samples for smoother graph
        if fps_history.len() > 120 {
            fps_history.pop_front();
        }

        // Prepare data for the FPS chart
        let fps_data: Vec<FpsDataPoint> = fps_history.iter().cloned().collect();
        drop(fps_history);
        drop(fps_sample_counter);

        // Calculate UI refresh consistency (FPS variance/stddev) for consistency tracking
        let ui_consistency_score = if fps_data.len() >= 10 {
            // Calculate standard deviation of last 30 FPS samples (or all if less)
            let sample_size = fps_data.len().min(30);
            let recent_fps: Vec<f64> = fps_data.iter()
                .rev()
                .take(sample_size)
                .map(|d| d.fps)
                .collect();
            
            let mean = recent_fps.iter().sum::<f64>() / recent_fps.len() as f64;
            let variance = recent_fps.iter()
                .map(|fps| (fps - mean).powi(2))
                .sum::<f64>() / recent_fps.len() as f64;
            variance.sqrt() // Standard deviation - lower is more consistent
        } else {
            0.0
        };

        // Update UI consistency history
        let mut ui_consistency_history = self.ui_consistency_history.borrow_mut();
        let mut ui_consistency_counter = self.ui_consistency_counter.borrow_mut();
        
        ui_consistency_history.push_back(UiConsistencyDataPoint {
            index: *ui_consistency_counter,
            consistency_score: ui_consistency_score,
        });
        *ui_consistency_counter += 1;
        
        if ui_consistency_history.len() > 120 {
            ui_consistency_history.pop_front();
        }
        
        let ui_consistency_data: Vec<UiConsistencyDataPoint> = ui_consistency_history.iter().cloned().collect();
        drop(ui_consistency_history);
        drop(ui_consistency_counter);

        // Update TPS history for rolling graph
        let mut tps_history = self.tps_history.borrow_mut();
        let mut tps_sample_counter = self.tps_sample_counter.borrow_mut();
        
        tps_history.push_back(TpsDataPoint {
            index: *tps_sample_counter,
            tps: game_tps as f64,
        });
        *tps_sample_counter += 1;
        
        // Keep only last 120 samples
        if tps_history.len() > 120 {
            tps_history.pop_front();
        }

        // Prepare data for the TPS chart
        let tps_data: Vec<TpsDataPoint> = tps_history.iter().cloned().collect();
        drop(tps_history);
        drop(tps_sample_counter);

        // Update Frame Time history for jitter analysis
        let mut frame_time_history = self.frame_time_history.borrow_mut();
        let mut frame_time_counter = self.frame_time_counter.borrow_mut();
        
        let frame_time_ms = pipeline_us as f64 / 1000.0;
        frame_time_history.push_back(FrameTimeDataPoint {
            index: *frame_time_counter,
            frame_time_ms,
        });
        *frame_time_counter += 1;
        
        if frame_time_history.len() > 120 {
            frame_time_history.pop_front();
        }
        
        let frame_time_data: Vec<FrameTimeDataPoint> = frame_time_history.iter().cloned().collect();
        drop(frame_time_history);
        drop(frame_time_counter);

        // Update Memory history
        let mut memory_history = self.memory_history.borrow_mut();
        let mut memory_counter = self.memory_counter.borrow_mut();
        
        memory_history.push_back(MemoryDataPoint {
            index: *memory_counter,
            memory_mb: memory_mb as f64,
        });
        *memory_counter += 1;
        
        if memory_history.len() > 120 {
            memory_history.pop_front();
        }
        
        let memory_data: Vec<MemoryDataPoint> = memory_history.iter().cloned().collect();
        drop(memory_history);
        drop(memory_counter);

        // Update Draw Calls history
        let mut draw_calls_history = self.draw_calls_history.borrow_mut();
        let mut draw_calls_counter = self.draw_calls_counter.borrow_mut();
        
        draw_calls_history.push_back(DrawCallsDataPoint {
            index: *draw_calls_counter,
            draw_calls: draw_calls as f64,
        });
        *draw_calls_counter += 1;
        
        if draw_calls_history.len() > 120 {
            draw_calls_history.pop_front();
        }
        
        let draw_calls_data: Vec<DrawCallsDataPoint> = draw_calls_history.iter().cloned().collect();
        drop(draw_calls_history);
        drop(draw_calls_counter);

        // Update Vertices history
        let mut vertices_history = self.vertices_history.borrow_mut();
        let mut vertices_counter = self.vertices_counter.borrow_mut();
        
        vertices_history.push_back(VerticesDataPoint {
            index: *vertices_counter,
            vertices: vertices_drawn as f64,
        });
        *vertices_counter += 1;
        
        if vertices_history.len() > 120 {
            vertices_history.pop_front();
        }
        
        let vertices_data: Vec<VerticesDataPoint> = vertices_history.iter().cloned().collect();
        drop(vertices_history);
        drop(vertices_counter);

        // Track input latency from the input thread (measured in microseconds, stored atomically)
        let input_latency_ms = self.input_state.input_latency_us.load(Ordering::Relaxed) as f64 / 1000.0;

        let mut input_latency_history = self.input_latency_history.borrow_mut();
        let mut input_latency_counter = self.input_latency_counter.borrow_mut();
        
        input_latency_history.push_back(InputLatencyDataPoint {
            index: *input_latency_counter,
            latency_ms: input_latency_ms,
        });
        *input_latency_counter += 1;
        
        if input_latency_history.len() > 120 {
            input_latency_history.pop_front();
        }
        
        let input_latency_data: Vec<InputLatencyDataPoint> = input_latency_history.iter().cloned().collect();
        drop(input_latency_history);
        drop(input_latency_counter);

        // Calculate frame time variance for jitter detection
        let frame_time_variance = if !frame_time_data.is_empty() {
            let mean = frame_time_data.iter().map(|d| d.frame_time_ms).sum::<f64>() / frame_time_data.len() as f64;
            let variance = frame_time_data.iter()
                .map(|d| (d.frame_time_ms - mean).powi(2))
                .sum::<f64>() / frame_time_data.len() as f64;
            variance.sqrt() // Standard deviation
        } else {
            0.0
        };

        v_flex()
            .gap_2()
            .p_2()
            .w_full()
            .bg(cx.theme().background.opacity(0.95))
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            // HEADER ROW - FPS indicators
            .child(
                h_flex()
                    .gap_3()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            // UI FPS
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(if ui_fps > 300.0 {
                                        cx.theme().success
                                    } else if ui_fps > 144.0 {
                                        cx.theme().warning
                                    } else {
                                        cx.theme().danger
                                    })
                                    .child(format!("UI: {:.0} FPS", ui_fps))
                            )
                            // Bevy/Renderer FPS
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(if bevy_fps > 200.0 {
                                        cx.theme().success
                                    } else if bevy_fps > 60.0 {
                                        cx.theme().warning
                                    } else {
                                        cx.theme().danger
                                    })
                                    .child(format!("Render: {:.0} FPS", bevy_fps))
                            )
                            // Pipeline time
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("Pipeline: {:.2}ms", pipeline_us as f64 / 1000.0))
                            )
                            .child({
                                let enabled = self.render_enabled.clone();
                                Button::new("toggle_render")
                                    .child(if self.render_enabled.load(std::sync::atomic::Ordering::Relaxed) {
                                        "⏸"
                                    } else {
                                        "▶"
                                    })
                                    
                                    .on_click(move |_event, _window, _cx| {
                                        let current = enabled.load(std::sync::atomic::Ordering::Relaxed);
                                        enabled.store(!current, std::sync::atomic::Ordering::Relaxed);
                                    })
                            })
                    )
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("collapse_performance")
                            .icon(IconName::X)
                            .ghost()
                            .on_click(move |_, _, _| {
                                state_clone.write().set_performance_overlay_collapsed(true);
                            })
                    })
            )
            // Metrics selector toolbar
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .p_1()
                    .flex_wrap()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_fps")
                            .icon(IconName::Activity)
                            .tooltip("FPS Graph")
                            
                            .selected(state.show_fps_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_fps_graph();
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_tps")
                            .icon(IconName::Play)
                            .tooltip("TPS Graph")
                            
                            .selected(state.show_tps_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_tps_graph();
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_frame_time")
                            .icon(IconName::Clock)
                            .tooltip("Frame Time")
                            
                            .selected(state.show_frame_time_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_frame_time_graph();
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_memory")
                            .icon(IconName::Database)
                            .tooltip("GPU Memory")
                            
                            .selected(state.show_memory_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_memory_graph();
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_draw_calls")
                            .icon(IconName::BookStack)
                            .tooltip("Draw Calls")
                            
                            .selected(state.show_draw_calls_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_draw_calls_graph();
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_vertices")
                            .icon(IconName::Triangle)
                            .tooltip("Vertices")
                            
                            .selected(state.show_vertices_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_vertices_graph();
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_input_latency")
                            .icon(IconName::CursorPointer)
                            .tooltip("Input Latency")
                            
                            .selected(state.show_input_latency_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_input_latency_graph();
                            })
                    })
                    .child({
                        let state_clone = state_arc.clone();
                        Button::new("toggle_ui_consistency")
                            .icon(IconName::GraphUp)
                            .tooltip("UI Consistency")
                            
                            .selected(state.show_ui_consistency_graph)
                            .on_click(move |_, _, _| {
                                state_clone.write().toggle_ui_consistency_graph();
                            })
                    })
            )
            .when(state.show_fps_graph && !fps_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .pt_2()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .mb_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child("FPS Graph")
                                )
                                .child({
                                    let fps_graph_clone = fps_graph_state.clone();
                                    
                                    ui::switch::Switch::new("fps_graph_type")
                                        .checked(*fps_graph_state.borrow())
                                        .label("Line")
                                        
                                        .on_click(move |checked, _, _| {
                                            *fps_graph_clone.borrow_mut() = *checked;
                                        })
                                })
                        )
                        .child(
                            div()
                                .h(px(100.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Area chart with semi-transparent fill (line mode)
                                    let theme = cx.theme();
                                    let stroke_color = theme.chart_1;
                                    let fill_color = stroke_color.opacity(0.2);
                                    
                                    AreaChart::new(fps_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.fps)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar chart with color coding: Green (high), Yellow (mid), Red (low)
                                    let theme = cx.theme();
                                    let success_color = theme.success;
                                    let warning_color = theme.warning;
                                    let danger_color = theme.danger;
                                    
                                    BarChart::new(fps_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.fps)
                                        .fill(move |d| {
                                            // Color code based on FPS:
                                            // Green: > 120 FPS (high)
                                            // Yellow: 60-120 FPS (mid)
                                            // Red: < 60 FPS (low)
                                            if d.fps >= 120.0 {
                                                success_color
                                            } else if d.fps >= 60.0 {
                                                warning_color
                                            } else {
                                                danger_color
                                            }
                                        })
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            // UI REFRESH CONSISTENCY GRAPH - Shows FPS variance over time (lower is better/more consistent)
            .when(state.show_ui_consistency_graph && !ui_consistency_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("📊 UI Refresh Consistency (StdDev) - Lower is smoother")
                        )
                        .child(
                            div()
                                .h(px(80.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Line mode - Area chart
                                    let theme = cx.theme();
                                    let stroke_color = theme.chart_2;
                                    let fill_color = stroke_color.opacity(0.2);

                                    AreaChart::new(ui_consistency_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.consistency_score)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar mode with color coding: Green (consistent), Yellow (moderate), Red (inconsistent)
                                    let theme = cx.theme();
                                    let success_color = theme.success;
                                    let warning_color = theme.warning;
                                    let danger_color = theme.danger;

                                    BarChart::new(ui_consistency_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.consistency_score)
                                        .fill(move |d| {
                                            // Color code based on consistency score (stddev):
                                            // Green: < 2.0 (very consistent)
                                            // Yellow: 2.0-5.0 (moderate jitter)
                                            // Red: > 5.0 (inconsistent/hitchy)
                                            if d.consistency_score < 2.0 {
                                                success_color
                                            } else if d.consistency_score < 5.0 {
                                                warning_color
                                            } else {
                                                danger_color
                                            }
                                        })
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            .child(
                // TPS Row - Add game thread monitoring
                h_flex()
                    .gap_2()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(if game_tps > 220.0 {
                                cx.theme().success
                            } else if game_tps > 120.0 {
                                cx.theme().warning
                            } else {
                                cx.theme().danger
                            })
                            .child(format!("🎮 {:.0} TPS", game_tps))
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Target: 240 | Ticks: {}", game_tick_count))
                    )
            )
            .when(state.show_tps_graph && !tps_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .mb_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child("TPS Graph (Game Thread)")
                                )
                        )
                        .child(
                            div()
                                .h(px(80.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Line mode - Area chart
                                    let theme = cx.theme();
                                    let stroke_color = theme.chart_2;
                                    let fill_color = stroke_color.opacity(0.2);
                                    
                                    AreaChart::new(tps_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.tps)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar mode
                                    let theme = cx.theme();
                                    let success_color = theme.success;
                                    let warning_color = theme.warning;
                                    let danger_color = theme.danger;
                                    
                                    BarChart::new(tps_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.tps)
                                        .fill(move |d| {
                                            // Color code based on TPS:
                                            // Green: > 220 TPS (high)
                                            // Yellow: 120-220 TPS (mid)
                                            // Red: < 120 TPS (low)
                                            if d.tps >= 220.0 {
                                                success_color
                                            } else if d.tps >= 120.0 {
                                                warning_color
                                            } else {
                                                danger_color
                                            }
                                        })
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            // DETAILED METRICS SECTION
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("⚡ Performance Details")
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .w_full()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .flex_1()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("GPU Time:")
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(if gpu_us < 8000 {
                                                        cx.theme().success
                                                    } else if gpu_us < 16000 {
                                                        cx.theme().warning
                                                    } else {
                                                        cx.theme().danger
                                                    })
                                                    .child(format!("{:.2}ms", gpu_us as f64 / 1000.0))
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("CPU Time:")
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(if cpu_us < 2000 {
                                                        cx.theme().success
                                                    } else if cpu_us < 5000 {
                                                        cx.theme().warning
                                                    } else {
                                                        cx.theme().danger
                                                    })
                                                    .child(format!("{:.2}ms", cpu_us as f64 / 1000.0))
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Frame Jitter:")
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(if frame_time_variance < 1.0 {
                                                        cx.theme().success
                                                    } else if frame_time_variance < 3.0 {
                                                        cx.theme().warning
                                                    } else {
                                                        cx.theme().danger
                                                    })
                                                    .child(format!("±{:.2}ms", frame_time_variance))
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Input Thread:")
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(if input_latency_ms < 5.0 {
                                                        cx.theme().success
                                                    } else if input_latency_ms < 15.0 {
                                                        cx.theme().warning
                                                    } else {
                                                        cx.theme().danger
                                                    })
                                                    .child(format!("{:.1}ms", input_latency_ms))
                                            )
                                    )
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .flex_1()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("GPU Memory:")
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().foreground)
                                                    .child(format!("{:.1}MB", memory_mb))
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Draw Calls:")
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().foreground)
                                                    .child(format!("{}", draw_calls))
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Vertices:")
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().foreground)
                                                    .child(format!("{}", vertices_drawn))
                                            )
                                    )
                            )
                    )
            )
            // FRAME TIME JITTER GRAPH - Critical for finding stutters!
            .when(state.show_frame_time_graph && !frame_time_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("📊 Frame Time (ms) - Spikes indicate stutters")
                        )
                        .child(
                            div()
                                .h(px(80.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Line mode - Area chart
                                    let theme = cx.theme();
                                    let stroke_color = theme.chart_3;
                                    let fill_color = stroke_color.opacity(0.2);
                                    
                                    AreaChart::new(frame_time_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.frame_time_ms)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar mode
                                    let theme = cx.theme();
                                    let chart_color = theme.chart_3;
                                    
                                    BarChart::new(frame_time_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.frame_time_ms)
                                        .fill(move |_d| chart_color)
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            // GPU MEMORY USAGE GRAPH
            .when(state.show_memory_graph && !memory_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("💾 GPU Memory Usage (MB)")
                        )
                        .child(
                            div()
                                .h(px(60.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Line mode - Area chart
                                    let theme = cx.theme();
                                    let stroke_color = theme.chart_4;
                                    let fill_color = stroke_color.opacity(0.2);
                                    
                                    AreaChart::new(memory_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.memory_mb)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar mode
                                    let theme = cx.theme();
                                    let chart_color = theme.chart_4;
                                    
                                    BarChart::new(memory_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.memory_mb)
                                        .fill(move |_d| chart_color)
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            // DRAW CALLS GRAPH
            .when(state.show_draw_calls_graph && !draw_calls_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("🎨 Draw Calls per Frame")
                        )
                        .child(
                            div()
                                .h(px(60.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Line mode - Area chart
                                    let theme = cx.theme();
                                    let stroke_color = theme.chart_5;
                                    let fill_color = stroke_color.opacity(0.2);
                                    
                                    AreaChart::new(draw_calls_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.draw_calls)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar mode
                                    let theme = cx.theme();
                                    let chart_color = theme.chart_5;
                                    
                                    BarChart::new(draw_calls_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.draw_calls)
                                        .fill(move |_d| chart_color)
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            // VERTICES GRAPH
            .when(state.show_vertices_graph && !vertices_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("🔺 Vertices Rendered")
                        )
                        .child(
                            div()
                                .h(px(60.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Line mode - Area chart
                                    let theme = cx.theme();
                                    let stroke_color = theme.chart_1;
                                    let fill_color = stroke_color.opacity(0.2);
                                    
                                    AreaChart::new(vertices_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.vertices)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar mode
                                    let theme = cx.theme();
                                    let chart_color = theme.chart_1;
                                    
                                    BarChart::new(vertices_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.vertices)
                                        .fill(move |_d| chart_color)
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            // INPUT LATENCY GRAPH - Critical for responsive controls! (Measured on input thread)
            .when(state.show_input_latency_graph && !input_latency_data.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .mt_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("⚡ Input Thread Latency (ms) - Time to send input to GPU")
                        )
                        .child(
                            div()
                                .h(px(60.))
                                .w_full()
                                .child(if *fps_graph_state.borrow() {
                                    // Line mode - Area chart
                                    let theme = cx.theme();
                                    let stroke_color = theme.warning;
                                    let fill_color = stroke_color.opacity(0.2);
                                    
                                    AreaChart::new(input_latency_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.latency_ms)
                                        .stroke(stroke_color)
                                        .fill(fill_color)
                                        .linear()
                                        .tick_margin(10)
                                        .into_any_element()
                                } else {
                                    // Bar mode
                                    let theme = cx.theme();
                                    let chart_color = theme.warning;
                                    
                                    BarChart::new(input_latency_data.clone())
                                        .x(|d| SharedString::from(format!("{}", d.index)))
                                        .y(|d| d.latency_ms)
                                        .fill(move |_d| chart_color)
                                        .tick_margin(10)
                                        .into_any_element()
                                })
                        )
                )
            })
            .into_any_element()
    }
}
