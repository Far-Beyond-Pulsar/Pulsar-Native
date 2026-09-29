//! Linux cursor control: Wayland or X11, chosen by the window's own handle.
//!
//! Same API as the Windows and macOS modules. A Wayland window uses the
//! Wayland bypass ([`super::wayland`]): relative deltas and a locked pointer,
//! no screen coordinates. Anything else uses X11 ([`super::x11`]): pointer
//! query and warp on one shared connection.

use super::{wayland, x11};

pub fn prepare_relative_mouse_mode() -> bool {
    true
}

/// Start of a drag: discard motion that piled up before it.
pub fn begin_relative_mouse_mode() {
    if wayland::is_active() {
        wayland::reset_delta();
    }
}

pub fn end_relative_mouse_mode() {}

/// Motion since the last call. Only meaningful when [`uses_relative_deltas`];
/// on X11 the input thread measures deltas from pointer positions instead.
pub fn take_mouse_delta() -> (f32, f32) {
    wayland::take_delta()
}

/// Whether mouse-look reads [`take_mouse_delta`] (Wayland) rather than
/// pointer positions plus a warp (X11).
pub fn uses_relative_deltas() -> bool {
    wayland::is_active()
}

/// Held keys when the platform can't be polled globally (Wayland); `None`
/// means poll with `device_query` (X11).
pub fn held_keys() -> Option<Vec<device_query::Keycode>> {
    wayland::is_active().then(wayland::held_keys)
}

pub fn lock_cursor_to_window(window: &gpui::Window) {
    if !wayland::lock(window) {
        x11::lock_cursor_to_window(window);
    }
}

pub fn lock_cursor_to_point(screen_x: i32, screen_y: i32, radius: i32) {
    if !wayland::is_active() {
        x11::lock_cursor_to_point(screen_x, screen_y, radius);
    }
}

pub fn unlock_cursor() {
    if wayland::is_active() {
        wayland::unlock();
    } else {
        x11::unlock_cursor();
    }
}

/// On Wayland the cursor is hidden by the viewport's `CursorStyle::None`
/// (applied by winit), so these only act on X11.
pub fn hide_cursor() {
    if !wayland::is_active() {
        x11::hide_cursor();
    }
}

pub fn show_cursor() {
    if !wayland::is_active() {
        x11::show_cursor();
    }
}

/// Wayland can't warp the pointer; while locked it doesn't move anyway.
pub fn set_cursor_position(screen_x: i32, screen_y: i32) {
    if !wayland::is_active() {
        x11::set_cursor_position(screen_x, screen_y);
    }
}

/// Wayland has no global pointer position.
pub fn get_cursor_position() -> Option<(i32, i32)> {
    if wayland::is_active() {
        None
    } else {
        x11::get_cursor_position()
    }
}

/// Wayland has no screen coordinates.
pub fn window_to_screen_position(
    window: &gpui::Window,
    window_x: f32,
    window_y: f32,
) -> Option<(i32, i32)> {
    if wayland::is_wayland_window(window) {
        None
    } else {
        x11::window_to_screen_position(window, window_x, window_y)
    }
}
