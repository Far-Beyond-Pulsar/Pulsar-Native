//! Keyboard input handling
//!
//! This module handles keyboard events and provides utilities for converting
//! between Winit keycodes and GPUI keystroke representations.

use gpui::*;
use winit::keyboard::KeyCode;
use winit::event::{ElementState, KeyEvent};
use crate::window::WinitGpuiApp;
use crate::window::convert_modifiers;
use winit::window::WindowId;

/// Convert a Winit KeyCode to a string representation for GPUI
///
/// Maps physical keycodes to their string representations used by GPUI's
/// keystroke system. Returns None for unsupported keys.
///
/// # Arguments
/// * `code` - The KeyCode to convert
///
/// # Returns
/// Some(String) if the key is supported, None otherwise
pub fn keycode_to_string(code: KeyCode) -> Option<String> {
    use KeyCode::*;
    Some(match code {
        // Letters
        KeyA => "a",
        KeyB => "b",
        KeyC => "c",
        KeyD => "d",
        KeyE => "e",
        KeyF => "f",
        KeyG => "g",
        KeyH => "h",
        KeyI => "i",
        KeyJ => "j",
        KeyK => "k",
        KeyL => "l",
        KeyM => "m",
        KeyN => "n",
        KeyO => "o",
        KeyP => "p",
        KeyQ => "q",
        KeyR => "r",
        KeyS => "s",
        KeyT => "t",
        KeyU => "u",
        KeyV => "v",
        KeyW => "w",
        KeyX => "x",
        KeyY => "y",
        KeyZ => "z",

        // Numbers
        Digit0 => "0",
        Digit1 => "1",
        Digit2 => "2",
        Digit3 => "3",
        Digit4 => "4",
        Digit5 => "5",
        Digit6 => "6",
        Digit7 => "7",
        Digit8 => "8",
        Digit9 => "9",

        // Special keys
        Space => "space",
        Enter => "enter",
        Tab => "tab",
        Backspace => "backspace",
        Escape => "escape",
        Delete => "delete",
        Insert => "insert",
        Home => "home",
        End => "end",
        PageUp => "pageup",
        PageDown => "pagedown",

        // Arrow keys
        ArrowUp => "up",
        ArrowDown => "down",
        ArrowLeft => "left",
        ArrowRight => "right",

        // Function keys
        F1 => "f1",
        F2 => "f2",
        F3 => "f3",
        F4 => "f4",
        F5 => "f5",
        F6 => "f6",
        F7 => "f7",
        F8 => "f8",
        F9 => "f9",
        F10 => "f10",
        F11 => "f11",
        F12 => "f12",

        // Punctuation and symbols
        Minus => "-",
        Equal => "=",
        BracketLeft => "[",
        BracketRight => "]",
        Backslash => "\\",
        Semicolon => ";",
        Quote => "'",
        Comma => ",",
        Period => ".",
        Slash => "/",
        Backquote => "`",

        _ => return None,
    }.to_string())
}

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
