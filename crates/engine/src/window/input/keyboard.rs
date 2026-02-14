//! Keyboard input handling
//!
//! This module handles keyboard events and provides utilities for converting
//! between Winit keycodes and GPUI keystroke representations.

use gpui::*;
use winit::event::{ElementState, KeyEvent};
use crate::window::WinitGpuiApp;
use winit::window::WindowId;
use super::conversion::keycode_to_string;

/// Handle keyboard input events
///
/// Processes keyboard events from Winit, converts them to GPUI keystrokes,
/// and forwards them to the appropriate GPUI window.
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window receiving the event
/// * `event` - The keyboard event from Winit
pub fn handle_keyboard_input(
    app: &mut WinitGpuiApp,
    window_id: WindowId,
    event: KeyEvent,
) {
    profiling::profile_scope!("Input::Keyboard");
    tracing::debug!("🎹 Keyboard event: {:?}, repeat: {}", event.physical_key, event.repeat);

    // Get the window state
    let Some(window_state) = app.windows.get_mut(&window_id) else {
        return;
    };

    // Forward keyboard events to GPUI
    if let Some(gpui_window_ref) = window_state.gpui_window.as_ref() {
        // gpui_window_ref.inject_input_event(cx, event)
        // Store event and create keystroke before borrowing
        let current_modifiers_val = window_state.current_modifiers;

        // Get the key string
        let keystroke_opt = match &event.physical_key {
            winit::keyboard::PhysicalKey::Code(code) => {
                if let Some(key) = keycode_to_string(*code) {
                    let key_char = match &event.text {
                        Some(text) if !text.is_empty() => Some(text.chars().next().map(|c| c.to_string()).unwrap_or_default()),
                        _ => None,
                    };

                    Some(Keystroke {
                        modifiers: current_modifiers_val,
                        key,
                        key_char,
                    })
                } else {
                    tracing::debug!("⚠️ Unsupported key code: {:?}", code);
                    None
                }
            }
            winit::keyboard::PhysicalKey::Unidentified(_) => None,
        };

        if let Some(keystroke) = keystroke_opt {
            let gpui_event = match event.state {
                ElementState::Pressed => {
                    tracing::debug!("🔽 KeyDown: {:?}", keystroke);

                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke,
                        is_held: event.repeat,
                    })
                }
                ElementState::Released => {
                    tracing::debug!("🔽 KeyUp: {:?}", keystroke);

                    PlatformInput::KeyUp(KeyUpEvent { keystroke })
                }
            };

            window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event)).unwrap();
        }
    }

    window_state.needs_render = true;
    window_state.winit_window.request_redraw();
}
