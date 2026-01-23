//! Keyboard modifier state handling
//!
//! This module handles changes to keyboard modifier states (Ctrl, Shift, Alt, etc.)
//! and forwards them to GPUI for proper event handling.

use gpui::*;
use winit::keyboard::ModifiersState;
use winit::window::WindowId;
use crate::window::{WinitGpuiApp, ToGpuiModifiers};

/// Handle keyboard modifier state changes
///
/// Updates the tracked modifier state and forwards the change to GPUI.
/// This ensures GPUI's event system has accurate modifier information for
/// keyboard shortcuts and other input handling.
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window receiving the event
/// * `new_modifiers` - The new modifier state from Winit
pub fn handle_modifiers_changed(
    app: &mut WinitGpuiApp,
    window_id: WindowId,
    new_modifiers: ModifiersState,
) {
    profiling::profile_scope!("Input::Modifiers");
    // Get the window state
    let Some(window_state) = app.windows.get_mut(&window_id) else {
        return;
    };

    // Update tracked modifiers (using extension trait)
    window_state.current_modifiers = new_modifiers.to_gpui();

    // Forward modifier changes to GPUI
    if let Some(gpui_window_ref) = window_state.gpui_window.as_ref() {
        let gpui_event = PlatformInput::ModifiersChanged(ModifiersChangedEvent {
            modifiers: window_state.current_modifiers,
            capslock: Capslock { on: false }, // TODO: Track capslock state
        });

        let _ = window_state.gpui_app.update(|cx| gpui_window_ref.inject_input_event(cx, gpui_event));
    }

    window_state.needs_render = true;
    window_state.winit_window.request_redraw();
}
