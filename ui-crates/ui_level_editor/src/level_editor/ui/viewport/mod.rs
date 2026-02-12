//! Viewport panel with 3D rendering and camera controls.
//!
//! This module provides the main viewport panel for the level editor, featuring:
//! - Zero-copy GPU rendering via Bevy
//! - Professional camera controls (FPS, pan, orbit, zoom)
//! - Performance monitoring and overlays
//! - Lock-free input processing on dedicated thread
//!
//! The viewport has been refactored into focused, reusable components for maintainability.

pub mod platform;
pub mod performance;
pub mod input_state;
pub mod components;

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use device_query::{DeviceQuery, DeviceState, Keycode};
use engine_backend::GameThread;
use gpui::*;
use ui::bevy_viewport::BevyViewport;
use ui::{h_flex, v_flex, ActiveTheme, StyledExt};
use ui_common::ViewportControls;
use ui::Sizable;

use super::actions::*;
use super::state::{CameraMode, LevelEditorState};
use components::camera_selector::render_camera_selector;
use components::gpu_pipeline_overlay::render_gpu_pipeline_overlay;
use components::performance_overlay::render_performance_overlay;
use components::viewport_options::render_viewport_options;
use input_state::InputState;
use crate::level_editor::ui::viewport::components::camera_selector::CameraSpeedControl;
use performance::*;

/// Viewport panel with zero-copy GPU rendering and professional camera controls.
///
/// This panel manages:
/// - Direct GPU rendering through Bevy (no CPU copies)
/// - Dedicated input thread for high-frequency polling
/// - Performance metrics tracking and visualization
/// - Camera mode selection and controls
/// - Visual option toggles (grid, wireframe, lighting)
pub struct ViewportPanel {
    /// Bevy viewport entity for GPU rendering
    viewport: Entity<BevyViewport>,

    /// Viewport controls state
    viewport_controls: ViewportControls,

    /// Render enable/disable flag
    render_enabled: Arc<AtomicBool>,

    /// Element bounds for coordinate conversion
    element_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,

    /// Performance metrics tracking
    metrics: RefCell<PerformanceMetrics>,

    /// Lock-free input state
    input_state: Arc<InputState>,

    /// Input thread spawn tracking
    input_thread_spawned: Arc<AtomicBool>,

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
        viewport: Entity<BevyViewport>,
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
            viewport_controls: ViewportControls::new(),
            render_enabled,
            element_bounds: Rc::new(RefCell::new(None)),
            metrics: RefCell::new(PerformanceMetrics::new()),
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
            keys_pressed: Rc::new(RefCell::new(HashSet::new())),
            alt_pressed: Rc::new(RefCell::new(false)),
            focus_handle,
        }
    }

    /// Render the viewport panel.
    pub fn render<V: 'static>(
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
        // Spawn dedicated input thread (once)
        self.spawn_input_thread_once(gpu_engine);

        // Update performance metrics
        self.update_performance_metrics(gpu_engine, game_thread);

        // Send input to GPU
        self.send_input_to_gpu(gpu_engine, state);

        // Build the viewport UI
        self.build_viewport_ui(state, state_arc, fps_graph_state, gpu_engine, game_thread, cx)
    }

    /// Spawn the input processing thread (only once).
    fn spawn_input_thread_once(&self, gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>) {
        if self.input_thread_spawned.load(Ordering::Relaxed) {
            return;
        }

        self.input_thread_spawned.store(true, Ordering::Relaxed);

        let input_state = self.input_state.clone();
        let gpu_engine_clone = gpu_engine.clone();
        let mouse_right_captured = self.mouse_right_captured.clone();
        let mouse_middle_captured = self.mouse_middle_captured.clone();
        let locked_cursor_x = self.locked_cursor_x.clone();
        let locked_cursor_y = self.locked_cursor_y.clone();

        std::thread::spawn(move || {
            profiling::set_thread_name("Input Thread");
            tracing::debug!("[INPUT-THREAD] 🚀 Dedicated RAW INPUT processing thread started");
            let device_state = DeviceState::new();
            let mut last_mouse_pos: Option<(i32, i32)> = None;

            loop {
                profiling::profile_scope!("input_poll");
                let input_start = std::time::Instant::now();
                std::thread::sleep(std::time::Duration::from_millis(8)); // ~120Hz

                let is_rotating = mouse_right_captured.load(Ordering::Acquire);
                let is_panning = mouse_middle_captured.load(Ordering::Acquire);

                if !is_rotating && !is_panning {
                    // Not active - clear state
                    last_mouse_pos = None;
                    input_state.set_forward(0);
                    input_state.set_right(0);
                    input_state.set_up(0);
                    input_state.set_boost(false);

                    // Clear GPU input
                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                        if let Some(ref helio_renderer) = engine.helio_renderer {
                            if let Ok(mut input) = helio_renderer.camera_input.try_lock() {
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

                    continue;
                }

                // Poll keyboard
                {
                    profiling::profile_scope!("keyboard_poll");
                    let keys: Vec<Keycode> = device_state.get_keys();
                    let forward = if keys.contains(&Keycode::W) {
                    1
                } else if keys.contains(&Keycode::S) {
                    -1
                } else {
                    0
                };
                let right = if keys.contains(&Keycode::D) {
                    1
                } else if keys.contains(&Keycode::A) {
                    -1
                } else {
                    0
                };
                let up = if keys.contains(&Keycode::E) || keys.contains(&Keycode::Space) {
                    1
                } else if keys.contains(&Keycode::Q)
                    || keys.contains(&Keycode::LShift)
                    || keys.contains(&Keycode::RShift)
                {
                    -1
                } else {
                    0
                };
                    let boost = keys.contains(&Keycode::LShift) || keys.contains(&Keycode::RShift);

                    input_state.set_forward(forward);
                    input_state.set_right(right);
                    input_state.set_up(up);
                    input_state.set_boost(boost);
                }

                // Poll mouse and calculate delta
                {
                    profiling::profile_scope!("mouse_poll");
                #[cfg(target_os = "windows")]
                {
                    let locked_screen_x = locked_cursor_x.load(Ordering::Relaxed);
                    let locked_screen_y = locked_cursor_y.load(Ordering::Relaxed);

                    if locked_screen_x > 0 && locked_screen_y > 0 {
                        use winapi::shared::windef::POINT;
                        use winapi::um::winuser::GetCursorPos;

                        unsafe {
                            let mut point = POINT { x: 0, y: 0 };
                            GetCursorPos(&mut point);

                            // Calculate delta from locked position (not last position)
                            let dx = point.x - locked_screen_x;
                            let dy = point.y - locked_screen_y;

                            if dx != 0 || dy != 0 {
                                // Send deltas DIRECTLY to renderer - zero latency path!
                                if let Ok(engine) = gpu_engine_clone.try_lock() {
                                    if let Some(ref helio_renderer) = engine.helio_renderer {
                                        if let Ok(mut input) = helio_renderer.camera_input.try_lock() {
                                            if is_rotating {
                                                input.mouse_delta_x = dx as f32;
                                                input.mouse_delta_y = dy as f32;
                                            } else if is_panning {
                                                input.pan_delta_x = dx as f32;
                                                input.pan_delta_y = dy as f32;
                                            }
                                        }
                                    }
                                }

                                // Also update atomics for UI feedback (optional)
                                if is_rotating {
                                    input_state.set_mouse_delta(dx as f32, dy as f32);
                                } else if is_panning {
                                    input_state.set_pan_delta(dx as f32, dy as f32);
                                }

                                // Reset cursor to locked position
                                platform::set_cursor_position(
                                    locked_screen_x,
                                    locked_screen_y,
                                );
                            }
                        }
                    }
                }
                }

                // Track latency
                let latency_us = input_start.elapsed().as_micros() as u64;
                input_state.set_input_latency_us(latency_us);
            }
        });
    }

    /// Update performance metrics from GPU.
    fn update_performance_metrics(&self, gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>, game_thread: &Arc<GameThread>) {
        if let Ok(engine) = gpu_engine.try_lock() {
            let ui_fps = engine.get_fps() as f64;
            let pipeline_us = engine.get_pipeline_time_us();
            
            let metrics_opt = engine.get_render_metrics();
            let (memory_mb, draw_calls, vertices) = if let Some(ref m) = metrics_opt {
                (m.memory_usage_mb, m.draw_calls, m.vertices_drawn)
            } else {
                (0.0, 0, 0)
            };

            let mut metrics = self.metrics.borrow_mut();
            
            // Update FPS (track both UI and Bevy)
            metrics.add_fps(ui_fps);
            
            // Update TPS from game thread
            let game_tps = game_thread.get_tps() as f64;
            metrics.add_tps(game_tps);
            
            // Update frame time
            let frame_time_ms = pipeline_us as f64 / 1000.0;
            metrics.add_frame_time(frame_time_ms);
            
            // Update memory
            metrics.add_memory(memory_mb as f64);
            
            // Update draw calls
            metrics.add_draw_calls(draw_calls as f64);
            
            // Update vertices
            metrics.add_vertices(vertices as f64);
            
            // Calculate UI consistency (FPS variance)
            if metrics.fps_history.len() >= 10 {
                let sample_size = metrics.fps_history.len().min(30);
                let recent_fps: Vec<f64> = metrics.fps_history.iter()
                    .rev()
                    .take(sample_size)
                    .map(|d| d.fps)
                    .collect();
                
                let mean = recent_fps.iter().sum::<f64>() / recent_fps.len() as f64;
                let variance = recent_fps.iter()
                    .map(|fps| (fps - mean).powi(2))
                    .sum::<f64>() / recent_fps.len() as f64;
                let std_dev = variance.sqrt();
                
                metrics.add_ui_consistency(std_dev);
            }
        }

        // Add input latency
        let latency_us = self.input_state.get_input_latency_us();
        self.metrics
            .borrow_mut()
            .add_input_latency(latency_us as f64 / 1000.0);
    }

    /// Send input state to GPU.
    /// NOTE: Mouse/pan deltas are now sent DIRECTLY from input thread for zero latency!
    /// This only handles WASD keys and zoom which don't need instant response.
    fn send_input_to_gpu(
        &self,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        state: &LevelEditorState,
    ) {
        if let Ok(engine) = gpu_engine.try_lock() {
            if let Some(ref helio_renderer) = engine.helio_renderer {
                if let Ok(mut input) = helio_renderer.camera_input.try_lock() {
                    // Update WASD keys and settings (these don't need instant response)
                    input.forward = self.input_state.get_forward() as f32;
                    input.right = self.input_state.get_right() as f32;
                    input.up = self.input_state.get_up() as f32;
                    input.boost = self.input_state.get_boost();
                    input.move_speed = self.input_state.get_move_speed();

                    // Zoom is also handled here since scroll events go through GPUI
                    input.zoom_delta = self.input_state.take_zoom_delta();

                    // NOTE: mouse_delta and pan_delta are now written DIRECTLY by input thread!
                    // We don't touch them here to avoid race conditions and latency.
                }
            }
        }
    }

    /// Build the complete viewport UI.
    fn build_viewport_ui<V: 'static>(
        &mut self,
        state: &mut LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        fps_graph_state: Rc<RefCell<bool>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        _game_thread: &Arc<GameThread>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let viewport_hovered = self.viewport_hovered.clone();
        let element_bounds = self.element_bounds.clone();
        let viewport_entity = self.viewport.clone();

        // Get performance data
        let (ui_fps, bevy_fps, pipeline_us) = if let Ok(engine) = gpu_engine.try_lock() {
            let ui_fps = engine.get_fps() as f64;
            let bevy_fps = engine.get_helio_fps() as f64;
            let pipeline = engine.get_pipeline_time_us();
            (ui_fps, bevy_fps, pipeline)
        } else {
            (0.0, 0.0, 0)
        };

        // Collect metric histories
        let metrics = self.metrics.borrow();
        let fps_data: Vec<FpsDataPoint> = metrics.fps_history.iter().cloned().collect();
        let tps_data: Vec<TpsDataPoint> = metrics.tps_history.iter().cloned().collect();
        let frame_time_data: Vec<FrameTimeDataPoint> = metrics.frame_time_history.iter().cloned().collect();
        let memory_data: Vec<MemoryDataPoint> = metrics.memory_history.iter().cloned().collect();
        let draw_calls_data: Vec<DrawCallsDataPoint> = metrics.draw_calls_history.iter().cloned().collect();
        let vertices_data: Vec<VerticesDataPoint> = metrics.vertices_history.iter().cloned().collect();
        let input_latency_data: Vec<InputLatencyDataPoint> = metrics.input_latency_history.iter().cloned().collect();
        let ui_consistency_data: Vec<UiConsistencyDataPoint> = metrics.ui_consistency_history.iter().cloned().collect();
        drop(metrics);

        // Clone for event handlers
        let input_state_scroll = Arc::clone(&self.input_state);
        let mouse_right_captured = self.mouse_right_captured.clone();
        let mouse_middle_captured = self.mouse_middle_captured.clone();
        let gpu_engine_for_click = gpu_engine.clone();
        let element_bounds_for_prepaint = self.element_bounds.clone();
        let element_bounds_for_click = self.element_bounds.clone();
        let state_arc_scroll = state_arc.clone();
        let gpu_engine_clone = gpu_engine.clone();
        let locked_cursor_x = self.locked_cursor_x.clone();
        let locked_cursor_y = self.locked_cursor_y.clone();
        let locked_cursor_screen_x = self.locked_cursor_screen_x.clone();
        let locked_cursor_screen_y = self.locked_cursor_screen_y.clone();

        // For mouse move tracking
        let element_bounds_move = self.element_bounds.clone();
        let gpu_engine_move = gpu_engine.clone();
        let last_mouse_pos = Rc::new(RefCell::new(None::<(f32, f32)>));

        // Main viewport container
        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .relative()
            // TRANSPARENT - no background! This creates the "hole" to see Bevy rendering
            .rounded(cx.theme().radius)
            // CRITICAL: Capture element bounds and update Bevy camera viewport
            .on_children_prepainted(move |children_bounds: Vec<Bounds<Pixels>>, _window, _cx| {
                if !children_bounds.is_empty() {
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
                    if let Ok(engine) = gpu_engine_clone.try_lock() {
                        if let Some(ref helio_renderer) = engine.helio_renderer {
                            if let Ok(mut camera_input) = helio_renderer.camera_input.try_lock() {
                                camera_input.viewport_x = min_x;
                                camera_input.viewport_y = min_y;
                                camera_input.viewport_width = max_x - min_x;
                                camera_input.viewport_height = max_y - min_y;
                            }
                        }
                    }
                }
            })
            // Track mouse movement and update Bevy input
            .on_mouse_move({
                let input_state_clone = self.input_state.clone();
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let state_arc_move = state_arc.clone();

                move |event, window, _cx| {
                    let is_rotating = mouse_right_captured.load(Ordering::Acquire);
                    let is_panning = mouse_middle_captured.load(Ordering::Acquire);

                    if is_rotating || is_panning {
                        #[cfg(target_os = "windows")]
                        {
                            // Cursor stays hidden on Windows
                        }

                        #[cfg(not(target_os = "windows"))]
                        {
                            let pos_x: f32 = event.position.x.into();
                            let pos_y: f32 = event.position.y.into();
                            let x = (pos_x * 1000.0) as i32;
                            let y = (pos_y * 1000.0) as i32;
                            input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                            input_state_clone.mouse_y.store(y, Ordering::Relaxed);
                        }

                        window.set_window_cursor_style(CursorStyle::None);
                    } else {
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        let x = (pos_x * 1000.0) as i32;
                        let y = (pos_y * 1000.0) as i32;
                        input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                        input_state_clone.mouse_y.store(y, Ordering::Relaxed);
                    }

                    // Handle overlay dragging
                    let mut state = state_arc_move.write();
                    if state.is_dragging_camera_overlay {
                        if let Some((start_x, start_y)) = state.camera_overlay_drag_start {
                            let current_x: f32 = event.position.x.into();
                            let current_y: f32 = event.position.y.into();
                            let delta_x = current_x - start_x;
                            let delta_y = current_y - start_y;

                            // Camera overlay positioned from right edge with .right(px(value))
                            // When dragging right, delta_x is positive, but right value should decrease
                            state.camera_overlay_pos.0 = (state.camera_overlay_pos.0 - delta_x).max(0.0);
                            state.camera_overlay_pos.1 = (state.camera_overlay_pos.1 + delta_y).max(0.0);
                            state.camera_overlay_drag_start = Some((current_x, current_y));
                        }
                        return;
                    }

                    if state.is_dragging_viewport_overlay {
                        if let Some((start_x, start_y)) = state.viewport_overlay_drag_start {
                            let current_x: f32 = event.position.x.into();
                            let current_y: f32 = event.position.y.into();
                            let delta_x = current_x - start_x;
                            let delta_y = current_y - start_y;

                            // Viewport overlay is positioned from left edge, normal drag
                            state.viewport_overlay_pos.0 = (state.viewport_overlay_pos.0 + delta_x).max(0.0);
                            state.viewport_overlay_pos.1 = (state.viewport_overlay_pos.1 + delta_y).max(0.0);
                            state.viewport_overlay_drag_start = Some((current_x, current_y));
                        }
                        return;
                    }
                    drop(state);

                    // Update Bevy mouse input
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
                        return;
                    };

                    let normalized_x = (element_x / viewport_width).clamp(0.0, 1.0);
                    let normalized_y = (element_y / viewport_height).clamp(0.0, 1.0);

                    let mut last_pos = last_mouse_pos.borrow_mut();
                    let (delta_x, delta_y) = if let Some((last_x, last_y)) = *last_pos {
                        (normalized_x - last_x, normalized_y - last_y)
                    } else {
                        (0.0, 0.0)
                    };

                    *last_pos = Some((normalized_x, normalized_y));
                    drop(last_pos);

                    if let Ok(engine) = gpu_engine_move.try_lock() {
                        if let Some(ref helio_renderer) = engine.helio_renderer {
                            let mut mouse_input = helio_renderer.viewport_mouse_input.lock();
                            mouse_input.mouse_pos.x = normalized_x;
                            mouse_input.mouse_pos.y = normalized_y;
                            mouse_input.mouse_delta.x = delta_x;
                            mouse_input.mouse_delta.y = delta_y;
                        }
                    }
                }
            })
            // Right-click for camera controls
            .on_mouse_down(gpui::MouseButton::Right, {
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_x = locked_cursor_x.clone();
                let locked_cursor_y = locked_cursor_y.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();
                let input_state_clone = self.input_state.clone();

                move |event, window, _cx| {
                    let shift_pressed = event.modifiers.shift;
                    let window_x: f32 = event.position.x.into();
                    let window_y: f32 = event.position.y.into();

                    if let Some((screen_x, screen_y)) = crate::level_editor::ui::viewport::platform::window_to_screen_position(window, window_x, window_y) {
                        locked_cursor_x.store(screen_x, Ordering::Relaxed);
                        locked_cursor_y.store(screen_y, Ordering::Relaxed);
                        locked_cursor_screen_x.store(screen_x, Ordering::Relaxed);
                        locked_cursor_screen_y.store(screen_y, Ordering::Relaxed);
                        crate::level_editor::ui::viewport::platform::lock_cursor_to_window(window);
                    } else {
                        crate::level_editor::ui::viewport::platform::lock_cursor_to_window(window);
                    }

                    let x = (window_x * 1000.0) as i32;
                    let y = (window_y * 1000.0) as i32;
                    input_state_clone.mouse_x.store(x, Ordering::Relaxed);
                    input_state_clone.mouse_y.store(y, Ordering::Relaxed);

                    if shift_pressed {
                        mouse_middle_captured.store(true, Ordering::Release);
                    } else {
                        mouse_right_captured.store(true, Ordering::Release);
                    }

                    crate::level_editor::ui::viewport::platform::hide_cursor();
                    window.set_window_cursor_style(CursorStyle::None);
                }
            })
            // Right-click release
            .on_mouse_up(gpui::MouseButton::Right, {
                let mouse_right_captured = mouse_right_captured.clone();
                let mouse_middle_captured = mouse_middle_captured.clone();
                let locked_cursor_screen_x = locked_cursor_screen_x.clone();
                let locked_cursor_screen_y = locked_cursor_screen_y.clone();

                move |_event, window, _cx| {
                    mouse_right_captured.store(false, Ordering::Release);
                    mouse_middle_captured.store(false, Ordering::Release);
                    locked_cursor_screen_x.store(0, Ordering::Relaxed);
                    locked_cursor_screen_y.store(0, Ordering::Relaxed);

                    crate::level_editor::ui::viewport::platform::show_cursor();
                    window.set_window_cursor_style(CursorStyle::Arrow);
                    crate::level_editor::ui::viewport::platform::unlock_cursor();
                }
            })
            // Scroll wheel for camera speed adjustment
            .on_scroll_wheel({
                let mouse_right_captured = mouse_right_captured.clone();
                let input_state_scroll = self.input_state.clone();
                
                move |event: &gpui::ScrollWheelEvent, _phase, _cx| {
                    let scroll_delta: f32 = event.delta.pixel_delta(px(1.0)).y.into();
                    
                    // Check if right-click is held (camera rotation mode)
                    let is_rotating = mouse_right_captured.load(Ordering::Acquire);
                    
                    if is_rotating {
                        // Right-click held: adjust camera move speed
                        let speed_delta = scroll_delta * 0.5; // Scale for reasonable adjustment
                        input_state_scroll.adjust_move_speed(speed_delta);
                        tracing::info!("[VIEWPORT] 🎚️ Camera speed adjusted by {:.2}", speed_delta);
                    }
                }
            })
            // Left-click for object selection
            .on_mouse_down(gpui::MouseButton::Left, {
                let gpu_engine_click = gpu_engine_for_click.clone();
                let element_bounds = element_bounds_for_click.clone();

                move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, _cx: &mut gpui::App| {
                    let bounds_opt = element_bounds.borrow();
                    let (element_x, element_y, gpui_width, gpui_height) = if let Some(ref bounds) = *bounds_opt {
                        let origin_x: f32 = bounds.origin.x.into();
                        let origin_y: f32 = bounds.origin.y.into();
                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        (pos_x - origin_x, pos_y - origin_y, width, height)
                    } else {
                        let window_size = window.viewport_size();
                        let pos_x: f32 = event.position.x.into();
                        let pos_y: f32 = event.position.y.into();
                        let width: f32 = window_size.width.into();
                        let height: f32 = window_size.height.into();
                        (pos_x, pos_y, width, height)
                    };

                    if let Ok(engine) = gpu_engine_click.try_lock() {
                        if let Some(ref helio_renderer) = engine.helio_renderer {
                            // The Bevy renderer draws to the full window (e.g. 1920x1080)
                            // while the GPUI viewport is just a "hole" in the UI that shows it
                            // We need to map from the click position within the GPUI viewport bounds
                            // to normalized coordinates (0-1) within the GPUI viewport area
                            let normalized_x = (element_x / gpui_width).clamp(0.0, 1.0);
                            let normalized_y = (element_y / gpui_height).clamp(0.0, 1.0);
                            
                            tracing::info!(
                                "[VIEWPORT] 🖱️ Left click:\n  Screen: ({}, {})\n  GPUI element: ({:.2}, {:.2}) in viewport {}x{}\n  Normalized: ({:.4}, {:.4})",
                                event.position.x, event.position.y, 
                                element_x, element_y, gpui_width, gpui_height,
                                normalized_x, normalized_y
                            );
                            
                            let mut mouse_input = helio_renderer.viewport_mouse_input.lock();
                            mouse_input.left_clicked = true;
                            mouse_input.left_down = true;
                            mouse_input.mouse_pos.x = normalized_x;
                            mouse_input.mouse_pos.y = normalized_y;
                            tracing::info!("[VIEWPORT] 🎯 Sent mouse input to Bevy: pos=({:.4}, {:.4}), clicked=true", normalized_x, normalized_y);
                        }
                    }
                }
            })
            // Left-click release
            .on_mouse_up(gpui::MouseButton::Left, {
                let gpu_engine_up = gpu_engine_for_click.clone();
                let state_arc_up = state_arc.clone();

                move |_event: &gpui::MouseUpEvent, _window: &mut gpui::Window, _cx: &mut gpui::App| {
                    let mut state = state_arc_up.write();
                    state.is_dragging_camera_overlay = false;
                    state.is_dragging_viewport_overlay = false;
                    state.camera_overlay_drag_start = None;
                    state.viewport_overlay_drag_start = None;
                    drop(state);

                    if let Ok(engine) = gpu_engine_up.try_lock() {
                        if let Some(ref helio_renderer) = engine.helio_renderer {
                            let mut mouse_input = helio_renderer.viewport_mouse_input.lock();
                            mouse_input.left_clicked = false;
                            mouse_input.left_down = false;
                        }
                    }
                }
            })
            .child({
                let input_state_speed = Arc::clone(&self.input_state);
                let mouse_right_captured_scroll = self.mouse_right_captured.clone();
                // Main viewport - Bevy renders through this transparent area
                div()
                    .flex()
                    .flex_1()
                    .size_full()
                    .on_scroll_wheel(move |event: &gpui::ScrollWheelEvent, _phase, cx| {
                        let scroll_delta: f32 = event.delta.pixel_delta(px(1.0)).y.into();
                        let is_rotating = mouse_right_captured_scroll.load(Ordering::Acquire);
                        
                        tracing::info!("[VIEWPORT] 📜 Scroll event received: delta={:.2}, is_rotating={}", scroll_delta, is_rotating);
                        
                        if is_rotating {
                            // Adjust camera movement speed when holding right-click
                            // Scroll up (positive) = increase speed, scroll down (negative) = decrease speed
                            let speed_delta = if scroll_delta > 0.0 { 2.0 } else if scroll_delta < 0.0 { -2.0 } else { 0.0 };
                            tracing::info!("[VIEWPORT] 🎚️ Scroll adjusting speed, ptr={:p}, delta={}", Arc::as_ptr(&input_state_speed), speed_delta);
                            input_state_speed.adjust_move_speed(speed_delta);
                            tracing::info!("[VIEWPORT] 🎚️ After adjust, speed={:.2}", input_state_speed.get_move_speed());
                            cx.stop_propagation();
                        } else {
                            // Normal zoom behavior when not holding right-click
                            input_state_scroll.set_zoom_delta(scroll_delta * 0.5);
                        }
                    })
                    .child(viewport_entity)
            })
            // Overlays
            .child(self.render_overlays(state, state_arc, fps_graph_state, ui_fps, bevy_fps, pipeline_us, fps_data, tps_data, frame_time_data, memory_data, draw_calls_data, vertices_data, input_latency_data, ui_consistency_data, gpu_engine, cx))
    }

    /// Render all viewport overlays.
    fn render_overlays<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        fps_graph_state: Rc<RefCell<bool>>,
        ui_fps: f64,
        bevy_fps: f64,
        pipeline_us: u64,
        fps_data: Vec<FpsDataPoint>,
        tps_data: Vec<TpsDataPoint>,
        frame_time_data: Vec<FrameTimeDataPoint>,
        memory_data: Vec<MemoryDataPoint>,
        draw_calls_data: Vec<DrawCallsDataPoint>,
        vertices_data: Vec<VerticesDataPoint>,
        input_latency_data: Vec<InputLatencyDataPoint>,
        ui_consistency_data: Vec<UiConsistencyDataPoint>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let mut overlays = v_flex()
            .size_full()
            .p_2()
            .gap_2()
            // Top-left: Viewport options
            .child(
                div()
                    .absolute()
                    .top(px(state.viewport_overlay_pos.1))
                    .left(px(state.viewport_overlay_pos.0))
                    .child(render_viewport_options(
                        state,
                        state_arc.clone(),
                        state.is_dragging_viewport_overlay,
                        cx,
                    )),
            );

        // Top-right: Camera selector
        if state.show_camera_mode_selector {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(state.camera_overlay_pos.1))
                    .right(px(state.camera_overlay_pos.0))
                    .child(render_camera_selector(
                        state,
                        state_arc.clone(),
                        state.camera_mode,
                        self.input_state.clone(),
                        state.is_dragging_camera_overlay,
                        cx,
                    )),
            );
        }

        // Bottom-left: Performance overlay
        if state.show_performance_overlay {
            overlays = overlays.child(
                div()
                    .absolute()
                    .bottom_2()
                    .left_2()
                    .max_w(px(400.0))
                    .child(render_performance_overlay(
                        state,
                        state_arc.clone(),
                        ui_fps,
                        bevy_fps,
                        pipeline_us,
                        fps_data,
                        tps_data,
                        frame_time_data,
                        memory_data,
                        draw_calls_data,
                        vertices_data,
                        input_latency_data,
                        ui_consistency_data,
                        fps_graph_state,
                        cx,
                    )),
            );
        }

        // GPU Pipeline overlay - positions next to performance overlay if both visible
        if state.show_gpu_pipeline_overlay {
            let overlay_div = if state.show_performance_overlay {
                // Position to the right of performance overlay
                div()
                    .absolute()
                    .bottom_2()
                    .left(px(300.0)) // 400px width + 10px gap
            } else {
                // Take performance overlay's position
                div()
                    .absolute()
                    .bottom_2()
                    .left_2()
            };

            overlays = overlays.child(
                overlay_div
                    .max_w(px(400.0))
                    .child(render_gpu_pipeline_overlay(
                        state,
                        state_arc.clone(),
                        gpu_engine,
                        cx,
                    )),
            );
        }

        // Initialization overlay - show when renderer is still warming up (< 10 FPS)
        if bevy_fps < 10.0 {
            overlays = overlays.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(cx.theme().background.opacity(0.9))
                    .child(
                        v_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                ui::spinner::Spinner::new()
                                    .with_size(ui::Size::Large)
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .text_color(cx.theme().foreground)
                                    .child("Initializing 3D Renderer...")
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("FPS: {:.1}", bevy_fps))
                            )
                    )
            );
        }

        overlays
    }
}
