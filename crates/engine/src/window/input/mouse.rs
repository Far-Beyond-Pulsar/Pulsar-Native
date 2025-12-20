//! Mouse input handling
//!
//! This module handles all mouse-related events including cursor movement,
//! button clicks, and scroll wheel events.

use gpui::*;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta};
use winit::window::WindowId;
use crate::window::{WinitGpuiApp, convert_mouse_button};

/// Handle cursor movement events
///
/// Updates cursor position tracking and forwards mouse move events to GPUI.
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
    // Get the window state
    let Some(window_state) = app.windows.get_mut(&window_id) else {
        return;
    };

    // Update cursor position tracking
    let scale_factor = window_state.winit_window.scale_factor() as f32;
    let logical_x = position.x as f32 / scale_factor;
    let logical_y = position.y as f32 / scale_factor;
    window_state.last_cursor_position = point(px(logical_x), px(logical_y));

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
/// and forwards mouse button events to GPUI.
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
    // Get the window state
    let Some(window_state) = app.windows.get_mut(&window_id) else {
        return;
    };

    // Forward mouse button events to GPUI
    if let Some(gpui_window_ref) = window_state.gpui_window.as_ref() {
        let gpui_button = convert_mouse_button(button);
        // Use actual cursor position for clicks, not smoothed position!
        let position = window_state.last_cursor_position;

        match state {
            ElementState::Pressed => {
                eprintln!("🖱️ MouseInput PRESSED: {:?} at {:?}", button, position);

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

                eprintln!("🔽 Injecting MouseDown event...");
                let result = window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                eprintln!("📊 MouseDown result: {:?}", result);
            }
            ElementState::Released => {
                eprintln!("🖱️ MouseInput RELEASED: {:?}", button);

                // Remove pressed button
                window_state.pressed_mouse_buttons.remove(&gpui_button);

                let gpui_event = PlatformInput::MouseUp(MouseUpEvent {
                    button: gpui_button,
                    position,
                    modifiers: window_state.current_modifiers,
                    click_count: window_state.click_state.current_count,
                });

                eprintln!("🔽 Injecting MouseUp event...");
                let result = window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
                eprintln!("📊 MouseUp result: {:?}", result);
            }
        }

        // Request redraw for click feedback
        window_state.needs_render = true;
        window_state.winit_window.request_redraw();
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
