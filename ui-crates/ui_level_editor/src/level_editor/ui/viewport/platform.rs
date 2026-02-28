//! Platform-specific cursor and input handling.
//!
//! This module provides cross-platform cursor locking, hiding, and positioning
//! for viewport camera controls. Each platform (Windows, macOS, Linux) has its
//! own implementation using native APIs for precise cursor control.

use gpui::Window;

/// Lock cursor to window bounds (prevents cursor from leaving the window).
///
/// This is used to keep the cursor confined during camera rotation to prevent
/// accidental clicks outside the viewport.
#[cfg(target_os = "windows")]
pub fn lock_cursor_to_window(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winapi::shared::minwindef::FALSE;
    use winapi::shared::windef::{POINT, RECT};
    use winapi::um::winuser::{ClientToScreen, ClipCursor, GetClientRect};

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::Win32(win32_handle) => unsafe {
                let hwnd = win32_handle.hwnd.get() as *mut winapi::shared::windef::HWND__;

                let mut client_rect = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };

                if GetClientRect(hwnd, &mut client_rect) != 0 {
                    let mut top_left = POINT {
                        x: client_rect.left,
                        y: client_rect.top,
                    };
                    let mut bottom_right = POINT {
                        x: client_rect.right,
                        y: client_rect.bottom,
                    };

                    ClientToScreen(hwnd, &mut top_left);
                    ClientToScreen(hwnd, &mut bottom_right);

                    let screen_rect = RECT {
                        left: top_left.x,
                        top: top_left.y,
                        right: bottom_right.x,
                        bottom: bottom_right.y,
                    };

                    ClipCursor(&screen_rect);
                    tracing::debug!("[VIEWPORT] 🔒 Cursor locked to window bounds");
                }
            },
            _ => {
                tracing::warn!("[VIEWPORT] Not a Win32 window handle");
            }
        },
        Err(e) => {
            tracing::error!("[VIEWPORT] Failed to get window handle: {:?}", e);
        }
    }
}

/// Lock cursor to a small area around a specific point.
///
/// This prevents the cursor from escaping during fast mouse movements,
/// which is critical for smooth camera rotation.
///
/// # Arguments
/// * `screen_x` - X coordinate in screen space
/// * `screen_y` - Y coordinate in screen space
/// * `radius` - Size of the confinement area in pixels
#[cfg(target_os = "windows")]
pub fn lock_cursor_to_point(screen_x: i32, screen_y: i32, radius: i32) {
    use winapi::shared::windef::RECT;
    use winapi::um::winuser::ClipCursor;

    unsafe {
        let screen_rect = RECT {
            left: screen_x - radius,
            top: screen_y - radius,
            right: screen_x + radius,
            bottom: screen_y + radius,
        };
        ClipCursor(&screen_rect);
        tracing::debug!(
            "[VIEWPORT] 🔒 Cursor confined to {}px radius around ({}, {})",
            radius,
            screen_x,
            screen_y
        );
    }
}

/// Release cursor confinement.
#[cfg(target_os = "windows")]
pub fn unlock_cursor() {
    use winapi::um::winuser::ClipCursor;

    unsafe {
        ClipCursor(std::ptr::null());
        tracing::debug!("[VIEWPORT] 🔓 Cursor unlocked");
    }
}

/// Hide the system cursor.
///
/// Windows uses a counter-based system, so we ensure the counter is negative.
#[cfg(target_os = "windows")]
pub fn hide_cursor() {
    use winapi::shared::minwindef::FALSE;
    use winapi::um::winuser::ShowCursor;

    unsafe {
        while ShowCursor(FALSE) >= 0 {}
        tracing::debug!("[VIEWPORT] 👻 Cursor hidden (Win32 ShowCursor)");
    }
}

/// Show the system cursor.
///
/// Windows uses a counter-based system, so we ensure the counter is non-negative.
#[cfg(target_os = "windows")]
pub fn show_cursor() {
    use winapi::um::winuser::ShowCursor;

    unsafe {
        while ShowCursor(1) < 0 {}
        tracing::debug!("[VIEWPORT] 👁️ Cursor shown (Win32 ShowCursor)");
    }
}

/// Set cursor to absolute screen position.
#[cfg(target_os = "windows")]
pub fn set_cursor_position(screen_x: i32, screen_y: i32) {
    use winapi::um::winuser::SetCursorPos;

    unsafe {
        SetCursorPos(screen_x, screen_y);
    }
}

/// Convert window-relative coordinates to screen coordinates.
///
/// # Returns
/// `Some((x, y))` if successful, `None` if the window handle is invalid.
#[cfg(target_os = "windows")]
pub fn window_to_screen_position(window: &Window, window_x: f32, window_y: f32) -> Option<(i32, i32)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::ClientToScreen;

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::Win32(win32_handle) => unsafe {
                let hwnd = win32_handle.hwnd.get() as *mut winapi::shared::windef::HWND__;
                let mut point = POINT {
                    x: window_x as i32,
                    y: window_y as i32,
                };
                ClientToScreen(hwnd, &mut point);
                Some((point.x, point.y))
            },
            _ => None,
        },
        Err(_) => None,
    }
}

// macOS implementations
#[cfg(target_os = "macos")]
pub fn set_cursor_position(screen_x: i32, screen_y: i32) {
    use core_graphics::display::CGDisplay;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    unsafe {
        CGDisplay::warp_mouse_cursor_position(core_graphics::geometry::CGPoint {
            x: screen_x as f64,
            y: screen_y as f64,
        })
        .ok();

        // Disassociate mouse and cursor position momentarily to prevent jumping
        CGEventSource::set_local_events_suppression_interval(0.0);
    }
}

#[cfg(target_os = "macos")]
pub fn window_to_screen_position(window: &Window, window_x: f32, window_y: f32) -> Option<(i32, i32)> {
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::NSPoint;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::AppKit(appkit_handle) => unsafe {
                let ns_window = appkit_handle.ns_window.as_ptr() as *mut AnyObject;
                let point = NSPoint {
                    x: window_x as f64,
                    y: window_y as f64,
                };
                let screen_point: NSPoint = msg_send![ns_window, convertPointToScreen: point];
                Some((screen_point.x as i32, screen_point.y as i32))
            },
            _ => None,
        },
        Err(_) => None,
    }
}

#[cfg(target_os = "macos")]
pub fn lock_cursor_to_window(_window: &Window) {
    // macOS doesn't support cursor confinement natively
    // We rely on cursor repositioning instead
}

#[cfg(target_os = "macos")]
pub fn lock_cursor_to_point(_screen_x: i32, _screen_y: i32, _radius: i32) {
    // No-op on macOS
}

#[cfg(target_os = "macos")]
pub fn unlock_cursor() {
    // No-op on macOS
}

#[cfg(target_os = "macos")]
pub fn hide_cursor() {
    // macOS cursor hiding is typically handled through GPUI/window system
}

#[cfg(target_os = "macos")]
pub fn show_cursor() {
    // macOS cursor showing is typically handled through GPUI/window system
}

// Linux/fallback implementations
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn set_cursor_position(_screen_x: i32, _screen_y: i32) {
    // Linux implementation would go here (X11/Wayland)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn window_to_screen_position(_window: &Window, _window_x: f32, _window_y: f32) -> Option<(i32, i32)> {
    None
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn hide_cursor() {
    // Linux implementation would go here
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn show_cursor() {
    // Linux implementation would go here
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn lock_cursor_to_window(_window: &Window) {
    // Linux implementation would go here
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn lock_cursor_to_point(_screen_x: i32, _screen_y: i32, _radius: i32) {
    // Linux implementation would go here
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn unlock_cursor() {
    // Linux implementation would go here
}
