//! Mouse input handling
//!
//! This module handles all mouse-related events including cursor movement,
//! button clicks, and scroll wheel events.

use gpui::*;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta};
use winit::window::{WindowId, ResizeDirection, CursorIcon};
use crate::window::{WinitGpuiApp, ToGpuiMouseButton};

// Constants for window manipulation (in logical pixels, will be scaled)
const TITLEBAR_HEIGHT_LOGICAL: f64 = 34.0;  // Match TitleBar::TITLE_BAR_HEIGHT
const RESIZE_BORDER: f64 = 8.0;     // Size of resize grip area in physical pixels
const WINDOW_CONTROLS_WIDTH_LOGICAL: f64 = 138.0;  // Width of min/max/close buttons (3 * 46px) in logical pixels

/// Determine if a position is in the titlebar drag area
/// All parameters should be in physical pixels
fn is_in_titlebar_drag_area(x: f64, y: f64, window_width: f64, scale_factor: f64, is_maximized: bool) -> bool {
    if is_maximized {
        return false;  // Can't drag maximized windows
    }

    let titlebar_height_physical = TITLEBAR_HEIGHT_LOGICAL * scale_factor;
    let controls_width_physical = WINDOW_CONTROLS_WIDTH_LOGICAL * scale_factor;

    // In titlebar height
    if y > titlebar_height_physical {
        return false;
    }

    // Not in window controls area (right side)
    if x > window_width - controls_width_physical {
        return false;
    }

    true
}

/// Determine resize direction based on cursor position
fn get_resize_direction(x: f64, y: f64, window_width: f64, window_height: f64, is_maximized: bool) -> Option<ResizeDirection> {
    if is_maximized {
        return None;  // Can't resize maximized windows
    }

    let on_left = x < RESIZE_BORDER;
    let on_right = x > window_width - RESIZE_BORDER;
    let on_top = y < RESIZE_BORDER;
    let on_bottom = y > window_height - RESIZE_BORDER;

    // Corners take priority
    if on_top && on_left {
        return Some(ResizeDirection::NorthWest);
    }
    if on_top && on_right {
        return Some(ResizeDirection::NorthEast);
    }
    if on_bottom && on_left {
        return Some(ResizeDirection::SouthWest);
    }
    if on_bottom && on_right {
        return Some(ResizeDirection::SouthEast);
    }

    // Edges
    if on_top {
        return Some(ResizeDirection::North);
    }
    if on_bottom {
        return Some(ResizeDirection::South);
    }
    if on_left {
        return Some(ResizeDirection::West);
    }
    if on_right {
        return Some(ResizeDirection::East);
    }

    None
}

/// Handle cursor movement events
///
/// Updates cursor position tracking, updates cursor icon for resize areas,
/// and forwards mouse move events to GPUI.
/// Includes information about which button is pressed during the move (for drag operations).
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window receiving the event
/// * `position` - Physical cursor position from Winit
pub fn handle_cursor_moved(
    app: &mut WinitGpuiApp,
    window_id: WindowId,
    position: PhysicalPosition<f64>,
) {
    profiling::profile_scope!("Input::CursorMoved");
    // Get the window state
    let Some(window_state) = app.windows.get_mut(&window_id) else {
        return;
    };

    // Update cursor position tracking
    let scale_factor = window_state.winit_window.scale_factor() as f32;
    let logical_x = position.x as f32 / scale_factor;
    let logical_y = position.y as f32 / scale_factor;
    window_state.last_cursor_position = point(px(logical_x), px(logical_y));

    // Update cursor icon based on position (for resize feedback)
    let size = window_state.winit_window.inner_size();
    let is_maximized = window_state.winit_window.is_maximized();

    if let Some(direction) = get_resize_direction(
        position.x,
        position.y,
        size.width as f64,
        size.height as f64,
        is_maximized,
    ) {
        // Set appropriate resize cursor
        let cursor = match direction {
            ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
            ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
        };
        window_state.winit_window.set_cursor_icon(cursor);
    } else {
        // Reset to default cursor
        window_state.winit_window.set_cursor_icon(CursorIcon::Default);
    }

    // Forward mouse move events to GPUI using inject_input_event
    if let Some(gpui_window_ref) = window_state.gpui_window.as_ref() {
        // Determine which button is pressed (if any)
        let pressed_button = if window_state.pressed_mouse_buttons.contains(&MouseButton::Left) {
            Some(MouseButton::Left)
        } else if window_state.pressed_mouse_buttons.contains(&MouseButton::Right) {
            Some(MouseButton::Right)
        } else if window_state.pressed_mouse_buttons.contains(&MouseButton::Middle) {
            Some(MouseButton::Middle)
        } else {
            None
        };

        let gpui_event = PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(logical_x), px(logical_y)),
            pressed_button,
            modifiers: window_state.current_modifiers,
        });

        let _ = window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));

        // Request redraw for cursor updates
        window_state.needs_render = true;
        window_state.winit_window.request_redraw();
    }
}

/// Handle mouse button events (clicks)
///
/// Tracks button press/release state, handles double-click detection,
/// handles window dragging and resizing, and forwards mouse button events to GPUI.
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window receiving the event
/// * `state` - Whether the button was pressed or released
/// * `button` - Which mouse button was affected
pub fn handle_mouse_input(
    app: &mut WinitGpuiApp,
    window_id: WindowId,
    state: ElementState,
    button: WinitMouseButton,
) {
    profiling::profile_scope!("Input::MouseButton");
    // Get the window state
    let Some(window_state) = app.windows.get_mut(&window_id) else {
        return;
    };

    // First, forward ALL mouse events to GPUI and check if it handles them
    let mut event_handled_by_gpui = false;

    if let Some(gpui_window_ref) = window_state.gpui_window.as_ref() {
        let gpui_button = button.to_gpui();
        let position = window_state.last_cursor_position;

        match state {
            ElementState::Pressed => {
                tracing::debug!("🖱️ MouseInput PRESSED: {:?} at {:?}", button, position);

                // Track pressed button
                window_state.pressed_mouse_buttons.insert(gpui_button);

                // Update click count
                let click_count = window_state.click_state.update(gpui_button, position);

                let gpui_event = PlatformInput::MouseDown(MouseDownEvent {
                    button: gpui_button,
                    position,
                    modifiers: window_state.current_modifiers,
                    click_count,
                    first_mouse: false,
                });

                tracing::debug!("🔽 Injecting MouseDown event...");
                let result = window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                tracing::debug!("📊 MouseDown result: {:?}", result);

                // Check if GPUI handled the event (e.g., button was clicked)
                // Event is handled if propagate is false (stopped) or default was prevented
                if let Ok(dispatch_result) = result {
                    event_handled_by_gpui = !dispatch_result.propagate || dispatch_result.default_prevented;
                }
            }
            ElementState::Released => {
                tracing::debug!("🖱️ MouseInput RELEASED: {:?}", button);

                // Remove pressed button
                window_state.pressed_mouse_buttons.remove(&gpui_button);

                let gpui_event = PlatformInput::MouseUp(MouseUpEvent {
                    button: gpui_button,
                    position,
                    modifiers: window_state.current_modifiers,
                    click_count: window_state.click_state.current_count,
                });

                tracing::debug!("🔽 Injecting MouseUp event...");
                let result = window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                tracing::debug!("📊 MouseUp result: {:?}", result);

                // Event is handled if propagate is false (stopped) or default was prevented
                if let Ok(dispatch_result) = result {
                    event_handled_by_gpui = !dispatch_result.propagate || dispatch_result.default_prevented;
                }
            }
        }

        // Request redraw for click feedback
        window_state.needs_render = true;
        window_state.winit_window.request_redraw();
    }

    // Only handle window manipulation if GPUI didn't consume the event
    // This allows titlebar buttons to "override" the drag handler
    if !event_handled_by_gpui && button == WinitMouseButton::Left && state == ElementState::Pressed {
        let position = window_state.last_cursor_position;
        let size = window_state.winit_window.inner_size();
        let scale_factor = window_state.winit_window.scale_factor();
        let is_maximized = window_state.winit_window.is_maximized();

        // Convert logical pixels (GPUI) to physical pixels (Winit) for comparison
        let pos_x: f32 = position.x.into();
        let pos_y: f32 = position.y.into();
        let pos_x_physical = (pos_x * scale_factor as f32) as f64;
        let pos_y_physical = (pos_y * scale_factor as f32) as f64;

        // Check for resize at window edges/corners (use physical pixels)
        if let Some(direction) = get_resize_direction(
            pos_x_physical,
            pos_y_physical,
            size.width as f64,
            size.height as f64,
            is_maximized,
        ) {
            // Start window resize
            if let Err(e) = window_state.winit_window.drag_resize_window(direction) {
                tracing::error!("❌ Failed to start window resize: {:?}", e);
            } else {
                tracing::debug!("🔲 Starting window resize: {:?}", direction);
            }
            return;
        }

        // Check for titlebar drag area (use physical pixels)
        if is_in_titlebar_drag_area(
            pos_x_physical,
            pos_y_physical,
            size.width as f64,
            scale_factor,
            is_maximized,
        ) {
            // Start window drag
            if let Err(e) = window_state.winit_window.drag_window() {
                tracing::error!("❌ Failed to start window drag: {:?}", e);
            } else {
                tracing::debug!("👆 Starting window drag from titlebar");
            }
        }
    }
}

/// Handle mouse wheel scroll events
///
/// Converts Winit scroll deltas to GPUI format and forwards scroll events.
/// Handles both line-based and pixel-based scrolling.
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window receiving the event
/// * `delta` - The scroll delta from Winit
pub fn handle_mouse_wheel(
    app: &mut WinitGpuiApp,
    window_id: WindowId,
    delta: MouseScrollDelta,
) {
    profiling::profile_scope!("Input::MouseWheel");
    // Get the window state
    let Some(window_state) = app.windows.get_mut(&window_id) else {
        return;
    };

    // Forward mouse wheel events to GPUI
    if let Some(gpui_window_ref) = window_state.gpui_window.as_ref() {
        let scale_factor = window_state.winit_window.scale_factor() as f32;

        // Convert delta
        let scroll_delta = match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                ScrollDelta::Lines(point(x, y))
            }
            MouseScrollDelta::PixelDelta(pos) => {
                ScrollDelta::Pixels(point(
                    px(pos.x as f32 / scale_factor),
                    px(pos.y as f32 / scale_factor),
                ))
            }
        };

        // Use actual cursor position for scroll events
        let position = window_state.last_cursor_position;

        let gpui_event = PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: scroll_delta,
            modifiers: window_state.current_modifiers,
            touch_phase: TouchPhase::Moved,
        });

        let _ = window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));

        // Request redraw for scroll updates
        window_state.needs_render = true;
        window_state.winit_window.request_redraw();
    }
}
